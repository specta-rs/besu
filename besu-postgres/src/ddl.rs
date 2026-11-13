use besu_core::{Dialect, TypeDef};
use besu_schema::{Column, Ddl, Domain, Enum, ForeignKey, Index, Sequence, View, view_body};

use crate::Postgres;

/// DDL generation for PostgreSQL.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDdl;

impl Ddl for PostgresDdl {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn quote(&self, ident: &str) -> String {
        let mut out = String::with_capacity(ident.len() + 2);
        out.push('"');
        for ch in ident.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        out
    }

    fn type_name(&self, ty: &TypeDef) -> String {
        Postgres::type_name(ty.clone()).into_owned()
    }

    fn normalise_type(&self, ty: &TypeDef) -> TypeDef {
        // PostgreSQL has no unsigned integers, so each `UInt` is stored as the signed type
        // that holds it. Folding them here is what makes a schema declaring `u16` converge
        // against the `INTEGER` the database reports back.
        match ty {
            TypeDef::UInt(8) => TypeDef::Int2,
            TypeDef::UInt(16) => TypeDef::Int4,
            TypeDef::UInt(32) => TypeDef::Int8,
            // `u64` lives in `NUMERIC(20,0)`, which is indistinguishable from a
            // `Decimal(20, 0)`; folding both together lets either converge.
            TypeDef::UInt(64) => TypeDef::Decimal(20, 0),
            // The same is true of `u128` in `NUMERIC(39,0)`. A signed `i128` is declared as
            // `Decimal(39, 0)` already, so the two share a type and converge.
            TypeDef::UInt(128) => TypeDef::Decimal(39, 0),
            // The fold has to reach through the wrappers: `Array(UInt(16))` is declared
            // `INTEGER[]`, which introspection reports as `Array(Int4)`.
            TypeDef::Array(inner) => TypeDef::Array(besu_core::types::TypeRef::owned(
                self.normalise_type(inner.as_type()),
            )),
            // A range's subtype is folded the same way, and a subtype with no built-in range
            // type falls back to `TEXT` exactly as `type_name` renders it. The precision of a
            // `Decimal` subtype is not part of `numrange`, so it folds away too.
            TypeDef::Range(inner) => {
                let inner = match self.normalise_type(inner.as_type()) {
                    TypeDef::Decimal(_, _) => TypeDef::Decimal(0, 0),
                    other => other,
                };
                match crate::dialect::range_type_name(&inner) {
                    Some(_) => TypeDef::Range(besu_core::types::TypeRef::owned(inner)),
                    None => TypeDef::Text,
                }
            }
            TypeDef::Collated(inner, collation) => {
                TypeDef::Collated(Box::new(self.normalise_type(inner)), collation.clone())
            }
            // A MySQL `SET` is a text label list here.
            TypeDef::Set(_) => TypeDef::Text,
            // A domain's base type is folded like any other type, so `Domain(UInt(16))` in a
            // snapshot compares equal to the `Domain(Int4)` introspection reports.
            TypeDef::Domain(def) => TypeDef::Domain(besu_core::DomainDef {
                name: def.name.clone(),
                base: besu_core::types::TypeRef::owned(self.normalise_type(def.base.as_type())),
            }),
            // PostGIS reports a geometry with SRID zero as one with no SRID at all, so
            // `Some(0)` folds away exactly as MySQL's does.
            TypeDef::Geometry { kind, srid } => TypeDef::Geometry {
                kind: kind.clone(),
                srid: srid.filter(|srid| *srid != 0),
            },
            TypeDef::Geography { kind, srid } => TypeDef::Geography {
                kind: kind.clone(),
                srid: srid.filter(|srid| *srid != 0),
            },
            // A named enum folds onto the `Custom(name)` a column of that type is introspected
            // as, so a declaration converges against the server's own report. An enum with
            // no name has no type for PostgreSQL to create, and renders as `TEXT`.
            TypeDef::Enum(def) if def.name.is_empty() => TypeDef::Text,
            TypeDef::Enum(def) => TypeDef::Custom(def.name.clone()),
            other => other.clone(),
        }
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!(
            "{} {}",
            self.quote(&column.name),
            self.type_name(&column.ty)
        );

        if let Some(generated) = &column.generated {
            // PostgreSQL only implements stored generated columns, so `VIRTUAL` is created
            // as `STORED`; the alternative would be a statement the server rejects.
            sql.push_str(" GENERATED ALWAYS AS (");
            sql.push_str(&generated.expression);
            sql.push_str(") STORED");
        } else if column.auto_increment {
            // PostgreSQL's identity columns are the closest thing to a portable
            // `AUTO_INCREMENT` and, unlike `SERIAL`, do not add a hidden default.
            sql.push_str(" GENERATED BY DEFAULT AS IDENTITY");
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        // A primary key is implicitly `NOT NULL`, so writing it again is noise.
        if !column.nullable && !column.primary_key {
            sql.push_str(" NOT NULL");
        }

        if column.unique && column.generated.is_none() {
            sql.push_str(" UNIQUE");
        }

        // A generated column's value comes from its expression, and PostgreSQL rejects a
        // default beside one.
        if column.generated.is_none()
            && let Some(default) = &column.default
        {
            sql.push_str(" DEFAULT ");
            sql.push_str(default);
        }

        sql
    }

    /// PostgreSQL can change a column in place, so only the parts that differ are emitted.
    ///
    /// Primary key and `UNIQUE` changes are not expressed here because they are *table*
    /// constraints; they need an explicit migration rather than an `ALTER COLUMN`. A
    /// generated column is the exception to "change it in place": its expression cannot be
    /// altered portably, and it holds no data of its own, so it is dropped and added.
    fn alter_column(&self, table: &str, from: &Column, to: &Column) -> Vec<String> {
        if from.generated != to.generated {
            return vec![
                self.drop_column(table, &from.name),
                self.add_column(table, to),
            ];
        }

        let table = self.quote_table(table);
        let column = self.quote(&to.name);
        let mut statements = Vec::new();

        if from.ty != to.ty {
            statements.push(format!(
                "ALTER TABLE {table} ALTER COLUMN {column} TYPE {}",
                self.type_name(&to.ty)
            ));
        }

        if from.nullable != to.nullable {
            if to.nullable {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP NOT NULL"
                ));
            } else {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} SET NOT NULL"
                ));
            }
        }

        if from.default != to.default {
            match &to.default {
                Some(default) => statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} SET DEFAULT {default}"
                )),
                None => statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP DEFAULT"
                )),
            }
        }

        if from.auto_increment != to.auto_increment {
            if to.auto_increment {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} \
                     ADD GENERATED BY DEFAULT AS IDENTITY"
                ));
            } else {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP IDENTITY"
                ));
            }
        }

        statements
    }

    /// PostgreSQL requires parentheses around an index expression unless it has the form of
    /// a function call. Wrapping an arithmetic expression is what makes it a statement the
    /// server accepts; a function call is left as written, which is also the spelling
    /// introspection reports back.
    fn index_expression_sql(&self, expression: &str) -> String {
        let expression = expression.trim();
        if expression.starts_with('(') || is_function_call(expression) {
            expression.to_owned()
        } else {
            format!("({expression})")
        }
    }

    fn drop_index(&self, table: &str, index: &Index) -> String {
        // An index belongs to the table's schema, so an unqualified `DROP INDEX` names the
        // wrong one whenever the table is not in the current schema.
        match table.split_once('.') {
            Some((schema, _)) => format!(
                "DROP INDEX {}.{}",
                self.quote(schema),
                self.quote(&index.name)
            ),
            None => format!("DROP INDEX {}", self.quote(&index.name)),
        }
    }

    fn drop_foreign_key(&self, table: &str, fk: &ForeignKey) -> String {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            self.quote_table(table),
            self.quote(&fk.name)
        )
    }

    /// `CREATE TYPE ... AS ENUM (...)`.
    fn create_enum(&self, e: &Enum) -> Vec<String> {
        let values: Vec<String> = e
            .values
            .iter()
            .map(|value| format!("'{}'", value.replace('\'', "''")))
            .collect();
        vec![format!(
            "CREATE TYPE {} AS ENUM ({})",
            self.quote_table(&e.name),
            values.join(", ")
        )]
    }

    /// `DROP TYPE`.
    fn drop_enum(&self, name: &str) -> Vec<String> {
        vec![format!("DROP TYPE {}", self.quote_table(name))]
    }

    /// PostgreSQL is the only dialect with domains.
    fn expresses_domains(&self) -> bool {
        true
    }

    /// `CREATE DOMAIN name AS base_type`.
    ///
    /// The constraints and default a domain can carry are not part of the IR, so they are not
    /// written here: a domain that already exists is left exactly as it is, and one created
    /// from a snapshot is built from its name and base type alone.
    fn create_domain(&self, domain: &Domain) -> Vec<String> {
        vec![format!(
            "CREATE DOMAIN {} AS {}",
            self.quote_table(&domain.name),
            self.type_name(&domain.base)
        )]
    }

    /// `DROP DOMAIN`.
    fn drop_domain(&self, name: &str) -> Vec<String> {
        vec![format!("DROP DOMAIN {}", self.quote_table(name))]
    }

    /// `ALTER TYPE ... ADD VALUE`.
    ///
    /// Before PostgreSQL 12 this cannot run inside a transaction block. `push` executes
    /// statements one at a time, so it works there; a migration file that wraps several
    /// statements in a transaction may need `ALTER TYPE` in its own migration.
    fn add_enum_value(&self, name: &str, value: &str) -> Vec<String> {
        vec![format!(
            "ALTER TYPE {} ADD VALUE '{}'",
            self.quote_table(name),
            value.replace('\'', "''")
        )]
    }

    /// `CREATE [MATERIALIZED] VIEW`.
    fn create_view(&self, view: &View) -> Vec<String> {
        let materialised = if view.materialised {
            "MATERIALIZED "
        } else {
            ""
        };
        vec![format!(
            "CREATE {materialised}VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// Replace a view, recreating a materialised one.
    ///
    /// PostgreSQL has no `CREATE OR REPLACE MATERIALIZED VIEW`, so a changed materialised
    /// view is dropped and created again. That re-runs its query, which is why the change is
    /// reported as `Risky`.
    fn replace_view(&self, view: &View) -> Vec<String> {
        if view.materialised {
            let mut statements = self.drop_view(&view.name, true);
            statements.extend(self.create_view(view));
            statements.extend(self.refresh_view(view));
            return statements;
        }
        vec![format!(
            "CREATE OR REPLACE VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// `DROP [MATERIALIZED] VIEW`.
    fn drop_view(&self, name: &str, materialised: bool) -> Vec<String> {
        let materialised = if materialised { "MATERIALIZED " } else { "" };
        vec![format!(
            "DROP {materialised}VIEW {}",
            self.quote_table(name)
        )]
    }

    /// `REFRESH MATERIALIZED VIEW`.
    fn refresh_view(&self, view: &View) -> Vec<String> {
        if !view.materialised {
            return Vec::new();
        }
        vec![format!(
            "REFRESH MATERIALIZED VIEW {}",
            self.quote_table(&view.name)
        )]
    }

    /// `CREATE SEQUENCE`.
    ///
    /// The options are spelled out so the generated statement describes exactly the sequence
    /// the IR holds; a bound of `None` is left to PostgreSQL's default for the data type.
    fn create_sequence(&self, sequence: &Sequence) -> Vec<String> {
        let mut sql = format!(
            "CREATE SEQUENCE {} AS {}",
            self.quote_table(&sequence.qualified_name()),
            sequence.data_type
        );
        sql.push_str(&format!(" INCREMENT BY {}", sequence.increment));
        if let Some(min) = sequence.min {
            sql.push_str(&format!(" MINVALUE {min}"));
        }
        if let Some(max) = sequence.max {
            sql.push_str(&format!(" MAXVALUE {max}"));
        }
        sql.push_str(&format!(" START WITH {}", sequence.start));
        sql.push_str(&format!(" CACHE {}", sequence.cache));
        sql.push_str(if sequence.cycle {
            " CYCLE"
        } else {
            " NO CYCLE"
        });
        vec![sql]
    }

    /// `ALTER SEQUENCE`.
    ///
    /// Every option is written, including the ones being reset to their default: there is no
    /// partial form that converges once a bound has been narrowed, so the statement describes
    /// the whole desired sequence.
    fn alter_sequence(&self, _from: &Sequence, to: &Sequence) -> Vec<String> {
        let mut sql = format!(
            "ALTER SEQUENCE {} AS {}",
            self.quote_table(&to.qualified_name()),
            to.data_type
        );
        sql.push_str(&format!(" INCREMENT BY {}", to.increment));
        match to.min {
            Some(min) => sql.push_str(&format!(" MINVALUE {min}")),
            None => sql.push_str(" NO MINVALUE"),
        }
        match to.max {
            Some(max) => sql.push_str(&format!(" MAXVALUE {max}")),
            None => sql.push_str(" NO MAXVALUE"),
        }
        sql.push_str(&format!(" START WITH {}", to.start));
        sql.push_str(&format!(" CACHE {}", to.cache));
        sql.push_str(if to.cycle { " CYCLE" } else { " NO CYCLE" });
        vec![sql]
    }

    /// `DROP SEQUENCE`.
    fn drop_sequence(&self, name: &str) -> Vec<String> {
        vec![format!("DROP SEQUENCE {}", self.quote_table(name))]
    }
}

/// Whether an expression has the form `name(...)`, which PostgreSQL accepts as an index key
/// without an extra pair of parentheses.
///
/// The name may be schema-qualified and quoted. Anything else — arithmetic, a cast, a
/// literal — needs the parentheses.
fn is_function_call(expression: &str) -> bool {
    let Some(open) = expression.find('(') else {
        return false;
    };
    let head = expression[..open].trim();
    !head.is_empty()
        && head
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '"' | '$'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use besu_schema::{IndexColumn, IndexKind};

    #[test]
    fn a_named_enum_renders_its_name_and_an_unnamed_one_is_text() {
        // PostgreSQL names the type, so the column's declaration is the type's name. An
        // inline enum from another dialect has nothing to create here and reads as text.
        assert_eq!(
            Postgres::type_name(TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy"]))),
            r#""mood""#
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Enum(besu_core::EnumDef::new("", &["happy"]))),
            "TEXT"
        );
    }

    #[test]
    fn a_named_enum_folds_onto_the_custom_type_introspection_reports() {
        let ddl = PostgresDdl;
        let declared = TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy", "sad"]));
        assert_eq!(
            Ddl::normalise_type(&ddl, &declared),
            TypeDef::Custom("mood".into())
        );
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Enum(besu_core::EnumDef::new("", &["happy", "sad"]))
            ),
            TypeDef::Text
        );
    }

    #[test]
    fn interval_network_and_bit_types_have_their_native_spellings() {
        assert_eq!(Postgres::type_name(TypeDef::Interval), "INTERVAL");
        assert_eq!(Postgres::type_name(TypeDef::Inet), "INET");
        assert_eq!(Postgres::type_name(TypeDef::Cidr), "CIDR");
        assert_eq!(Postgres::type_name(TypeDef::Bit(5)), "BIT(5)");
        assert_eq!(Postgres::type_name(TypeDef::VarBit(0)), "BIT VARYING");
        assert_eq!(Postgres::type_name(TypeDef::VarBit(7)), "BIT VARYING(7)");
        // MySQL's `SET` has no PostgreSQL spelling, so it is text and folds the same way.
        assert_eq!(
            Postgres::type_name(TypeDef::Set(besu_core::EnumLabels::from_static(&["a"]))),
            "TEXT"
        );
    }

    #[test]
    fn a_domain_renders_its_name_and_its_base_folds() {
        let ddl = PostgresDdl;
        let domain = TypeDef::Domain(besu_core::DomainDef::new("positive", &TypeDef::Int4));
        assert_eq!(Postgres::type_name(domain.clone()), r#""positive""#);
        assert_eq!(Ddl::normalise_type(&ddl, &domain), domain);

        // A snapshot declaring `u16` as the base of a domain has to converge against the
        // `integer` the database reports for it.
        let declared = TypeDef::Domain(besu_core::DomainDef::new("small", &TypeDef::UInt(16)));
        assert_eq!(
            Ddl::normalise_type(&ddl, &declared),
            TypeDef::Domain(besu_core::DomainDef::new("small", &TypeDef::Int4))
        );
    }

    #[test]
    fn ranges_and_spatial_columns_render_their_postgis_spellings() {
        use besu_core::GeometryKind;
        use besu_core::types::TypeRef;

        let range = |element: TypeDef| TypeDef::Range(TypeRef::owned(element));
        assert_eq!(Postgres::type_name(range(TypeDef::Int4)), "int4range");
        assert_eq!(Postgres::type_name(range(TypeDef::Int8)), "int8range");
        assert_eq!(
            Postgres::type_name(range(TypeDef::Decimal(10, 2))),
            "numrange",
            "numrange's subtype is an unconstrained numeric, whatever the marker says"
        );
        assert_eq!(Postgres::type_name(range(TypeDef::Timestamp)), "tsrange");
        assert_eq!(
            Postgres::type_name(range(TypeDef::TimestampTz)),
            "tstzrange"
        );
        assert_eq!(Postgres::type_name(range(TypeDef::Date)), "daterange");
        // A subtype PostgreSQL has no built-in range for is stored as text, exactly as the
        // fold says.
        assert_eq!(Postgres::type_name(range(TypeDef::Text)), "TEXT");
        assert_eq!(
            Postgres::type_name(TypeDef::Array(TypeRef::owned(range(TypeDef::Int4)))),
            "int4range[]"
        );

        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Point,
                srid: Some(4326),
            }),
            "GEOMETRY(POINT,4326)"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Geometry,
                srid: None,
            }),
            "GEOMETRY"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geography {
                kind: GeometryKind::MultiPolygon,
                srid: Some(3857),
            }),
            "GEOGRAPHY(MULTIPOLYGON,3857)"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Other("PointZ".into()),
                srid: None,
            }),
            "GEOMETRY(PointZ)"
        );

        assert_eq!(Postgres::type_name(TypeDef::Vector(3)), "VECTOR(3)");
        assert_eq!(Postgres::type_name(TypeDef::Vector(0)), "VECTOR");
    }

    #[test]
    fn a_range_and_a_vector_declaration_fold_onto_what_introspection_reports() {
        use besu_core::types::TypeRef;

        let ddl = PostgresDdl;
        // `numrange`'s subtype is an unconstrained numeric, so the marker's precision and
        // scale cannot survive a push/pull and fold away.
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Range(TypeRef::owned(TypeDef::Decimal(10, 2)))
            ),
            TypeDef::Range(TypeRef::owned(TypeDef::Decimal(0, 0)))
        );
        // A range with no PostgreSQL spelling is stored as text.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Range(TypeRef::owned(TypeDef::Text))),
            TypeDef::Text
        );
        // The unsigned markers fold inside the subtype, the same as they do in an array.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Range(TypeRef::owned(TypeDef::UInt(32)))),
            TypeDef::Range(TypeRef::owned(TypeDef::Int8))
        );
        // PostGIS has no SRID zero: a column declared with one converges against the plain
        // shape introspection reports.
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Geometry {
                    kind: besu_core::GeometryKind::Point,
                    srid: Some(0),
                }
            ),
            TypeDef::Geometry {
                kind: besu_core::GeometryKind::Point,
                srid: None,
            }
        );
        // A vector's dimension is part of its type, and both are already what is reported.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Vector(3)),
            TypeDef::Vector(3)
        );
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Vector(0)),
            TypeDef::Vector(0)
        );
    }

    #[test]
    fn a_domain_is_created_with_its_name_and_base() {
        let ddl = PostgresDdl;
        let domain = besu_schema::Domain {
            name: "positive".into(),
            base: TypeDef::Int4,
        };
        assert_eq!(
            Ddl::create_domain(&ddl, &domain),
            [r#"CREATE DOMAIN "positive" AS INTEGER"#]
        );
        assert_eq!(
            Ddl::drop_domain(&ddl, "positive"),
            [r#"DROP DOMAIN "positive""#]
        );
    }

    #[test]
    fn an_arithmetic_expression_is_parenthesised_and_a_function_call_is_not() {
        let index = Index {
            name: "events_score_idx".into(),
            columns: vec![
                IndexColumn::expression("a + b"),
                IndexColumn::expression("lower(name)"),
            ],
            unique: false,
            kind: IndexKind::Btree,
            predicate: None,
        };

        assert_eq!(
            PostgresDdl.create_index("events", &index),
            r#"CREATE INDEX "events_score_idx" ON "events" ((a + b), lower(name))"#
        );
    }
}
