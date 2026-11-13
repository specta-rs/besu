use std::collections::BTreeMap;

use besu_core::TypeDef;
use besu_core::schema::{IndexKind, ReferenceAction};
use besu_core::types::{DomainDef, TypeRef};
use besu_schema::{
    Check, Column, Domain, Enum, ForeignKey, Generated, Index, IndexColumn, Schema, Sequence,
    Table, Unique, View, normalise_expression, normalise_generated_expression,
};

use crate::Postgres;

/// Read the database's schema from PostgreSQL's own catalogue.
///
/// Every base table outside PostgreSQL's system schemas is reported. A table in the
/// connection's `current_schema()` is reported unqualified (`schema: None`); a table
/// anywhere else carries its schema, because its identity is `(schema, name)`. Besu's own
/// `_besu_migrations` bookkeeping table is skipped.
///
/// Objects Besu cannot model are skipped rather than reported as a lossy version of
/// themselves, because a `push` that sees them would try to drop or recreate them:
///
/// - indexes with `INCLUDE` columns, which the IR cannot name,
/// - indexes that back a constraint (exclusion constraints in particular), which the
///   constraint owns and `DROP INDEX` cannot remove,
/// - indexes using an access method the IR has no name for (`hash`, `spgist`),
/// - triggers, which are never read at all.
///
/// Views are read into [`Schema::views`] and standalone sequences into
/// [`Schema::sequences`], so `besu pull` records what is really there.
pub async fn introspect<Dr>(db: &besu::Database<Dr>) -> Result<Schema, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let current = current_schema(db).await?;

    let names = db
        .execute(besu::sql!(
            "SELECT t.table_schema, t.table_name \
             FROM information_schema.tables t \
             WHERE t.table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND t.table_schema NOT LIKE 'pg_toast%' \
               AND t.table_schema NOT LIKE 'pg_temp%' \
               AND t.table_type = 'BASE TABLE' \
               AND t.table_name <> '_besu_migrations' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   JOIN pg_class c ON c.oid = d.objid \
                   JOIN pg_namespace n ON n.oid = c.relnamespace \
                   WHERE d.deptype = 'e' \
                     AND n.nspname = t.table_schema AND c.relname = t.table_name \
               ) \
             ORDER BY t.table_schema, t.table_name"
        ))
        .all::<(String, String)>()
        .await?;

    let mut tables: BTreeMap<String, Table> = names
        .into_iter()
        .map(|(schema, name)| {
            let key = qualified(&schema, &name, current.as_deref());
            let table = Table {
                name,
                // The default schema is implicit: naming it would make every declared
                // table, which leaves `schema` unset, look like a different table.
                schema: (Some(schema.as_str()) != current.as_deref()).then_some(schema),
                columns: Vec::new(),
                indexes: Vec::new(),
                uniques: Vec::new(),
                foreign_keys: Vec::new(),
                checks: Vec::new(),
                comment: None,
            };
            (key, table)
        })
        .collect();

    introspect_columns(db, &mut tables, current.as_deref()).await?;
    introspect_generated_columns(db, &mut tables, current.as_deref()).await?;
    introspect_constraints(db, &mut tables, current.as_deref()).await?;
    introspect_checks(db, &mut tables, current.as_deref()).await?;
    introspect_indexes(db, &mut tables, current.as_deref()).await?;
    introspect_foreign_keys(db, &mut tables, current.as_deref()).await?;

    let mut schema = Schema::new();
    schema.tables = tables.into_values().collect();
    schema.enums = introspect_enums(db, current.as_deref()).await?;
    schema.domains = introspect_domains(db, current.as_deref()).await?;
    schema.views = introspect_views(db, current.as_deref()).await?;
    schema.sequences = introspect_sequences(db, current.as_deref()).await?;
    schema.sort();
    Ok(schema)
}

/// Qualify a name with its schema, unless that schema is the connection's default.
///
/// Everything the IR names — tables, enum types, the types columns are declared with — uses
/// the same rule, so `(schema, name)` is the identity and the default schema stays implicit.
fn qualified(schema: &str, name: &str, current: Option<&str>) -> String {
    if Some(schema) == current {
        name.to_owned()
    } else {
        format!("{schema}.{name}")
    }
}

/// The schema unqualified names resolve to, or `None` when `search_path` is empty.
async fn current_schema<Dr>(db: &besu::Database<Dr>) -> Result<Option<String>, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    Ok(db
        .execute(besu::sql!("SELECT current_schema()"))
        .all::<Option<String>>()
        .await?
        .into_iter()
        .next()
        .flatten())
}

/// Read every named enum type into [`Schema::enums`].
///
/// Enum order matters in PostgreSQL — it is the order values compare in — so the values are
/// collected by `enumsortorder`. An enum outside the default schema is named
/// `schema.type`, the same way a table elsewhere is qualified, so two schemas can each
/// define a `mood` without being confused for each other.
async fn introspect_enums<Dr>(
    db: &besu::Database<Dr>,
    current: Option<&str>,
) -> Result<Vec<Enum>, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, t.typname, e.enumlabel \
             FROM pg_type t \
             JOIN pg_enum e ON e.enumtypid = t.oid \
             JOIN pg_namespace n ON n.oid = t.typnamespace \
             WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
             ORDER BY n.nspname, t.typname, e.enumsortorder"
        ))
        .all::<(String, String, String)>()
        .await?;

    let mut enums: Vec<Enum> = Vec::new();
    for (schema, name, value) in rows {
        let name = qualified(&schema, &name, current);
        match enums.iter_mut().find(|e| e.name == name) {
            Some(existing) => existing.values.push(value),
            None => enums.push(Enum {
                name,
                values: vec![value],
            }),
        }
    }

    Ok(enums)
}

/// Read every domain into [`Schema::domains`].
///
/// A domain's name is qualified the same way an enum's is, and its base type goes through the
/// same parser a column does, so the two descriptions are the same [`TypeDef`]. A domain
/// built on another domain is read with the inner domain's name as its base, which is a
/// [`TypeDef::Custom`]; the wire format is still the innermost base's, so values decode.
async fn introspect_domains<Dr>(
    db: &besu::Database<Dr>,
    current: Option<&str>,
) -> Result<Vec<Domain>, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, t.typname, bt.typtype::text, bn.nspname, bt.typname, \
                    format_type(t.typbasetype, t.typtypmod) \
             FROM pg_type t \
             JOIN pg_namespace n ON n.oid = t.typnamespace \
             JOIN pg_type bt ON bt.oid = t.typbasetype \
             JOIN pg_namespace bn ON bn.oid = bt.typnamespace \
             WHERE t.typtype = 'd' \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
             ORDER BY n.nspname, t.typname"
        ))
        .all::<(String, String, String, String, String, String)>()
        .await?;

    Ok(rows
        .into_iter()
        .map(
            |(schema, name, base_typtype, base_schema, base_name, formatted)| Domain {
                name: qualified(&schema, &name, current),
                base: parse_named_type(
                    &base_typtype,
                    &base_name,
                    &base_schema,
                    &formatted,
                    current,
                ),
            },
        )
        .collect())
}

/// Read every view, materialised or not, into [`Schema::views`].
///
/// `pg_class.relkind` is the only catalogue that distinguishes a materialised view from a
/// plain one; `information_schema.views` lists only the plain ones. `pg_get_viewdef` is the
/// body, and a view whose definition cannot be read (the owner may have revoked it) is
/// skipped rather than reported with an empty body.
async fn introspect_views<Dr>(
    db: &besu::Database<Dr>,
    current: Option<&str>,
) -> Result<Vec<View>, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, c.relname, c.relkind = 'm', pg_get_viewdef(c.oid) \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('v', 'm') \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
               AND n.nspname NOT LIKE 'pg_temp%' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   WHERE d.classid = 'pg_class'::regclass AND d.objid = c.oid \
                     AND d.deptype = 'e' \
               ) \
             ORDER BY n.nspname, c.relname"
        ))
        .all::<(String, String, bool, Option<String>)>()
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(schema, name, materialised, definition)| {
            Some(View {
                name: qualified(&schema, &name, current),
                definition: definition?,
                materialised,
            })
        })
        .collect())
}

/// Read every standalone sequence into [`Schema::sequences`].
///
/// A sequence that a `SERIAL` or identity column owns is skipped: it belongs to that column,
/// which is modelled by [`Column::auto_increment`], and reporting it would make `push` try
/// to create or drop a sequence the column needs. `pg_depend` is what distinguishes the two:
/// an auto dependency (`a`) is a `SERIAL`, an internal one (`i`) an identity column.
///
/// `pg_sequences` reports a column as `NULL` when the connected role cannot inspect the
/// sequence, so such a row is skipped rather than guessed at.
async fn introspect_sequences<Dr>(
    db: &besu::Database<Dr>,
    current: Option<&str>,
) -> Result<Vec<Sequence>, Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT s.schemaname, s.sequencename, s.data_type::text, s.start_value, \
                    s.min_value, s.max_value, s.increment_by, s.cycle, s.cache_size \
             FROM pg_sequences s \
             JOIN pg_class c ON c.relname = s.sequencename \
             JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = s.schemaname \
             WHERE s.schemaname NOT IN ('pg_catalog', 'information_schema') \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   WHERE d.objid = c.oid \
                     AND d.classid = 'pg_class'::regclass \
                     AND d.refclassid = 'pg_class'::regclass \
                     AND d.deptype IN ('a', 'i') \
               ) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_depend d \
                   WHERE d.objid = c.oid \
                     AND d.classid = 'pg_class'::regclass \
                     AND d.deptype = 'e' \
               ) \
             ORDER BY s.schemaname, s.sequencename"
        ))
        .all::<(
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            bool,
            Option<i64>,
        )>()
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(schema, name, data_type, start, min, max, increment, cycle, cache)| {
                Some(
                    Sequence {
                        name,
                        schema: (Some(schema.as_str()) != current).then_some(schema),
                        data_type: data_type.unwrap_or_else(|| "bigint".into()),
                        start: start?,
                        increment: increment?,
                        min,
                        max,
                        cycle,
                        cache: cache?,
                    }
                    .normalised(),
                )
            },
        )
        .collect())
}

async fn introspect_columns<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT table_schema, table_name, column_name, is_nullable, column_default, \
                    is_identity \
             FROM information_schema.columns \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND table_schema NOT LIKE 'pg_toast%' \
             ORDER BY table_schema, table_name, ordinal_position"
        ))
        .all::<(String, String, String, String, Option<String>, String)>()
        .await?;

    for (schema, table_name, name, is_nullable, default, is_identity) in rows {
        let Some(table) = tables.get_mut(&qualified(&schema, &table_name, current)) else {
            continue;
        };

        // A `SERIAL` column sequesters its sequence in the default, while a modern identity
        // column reports `is_identity`. Both mean "the database generates this value".
        let auto_increment = is_identity == "YES"
            || default
                .as_deref()
                .is_some_and(|default| default.starts_with("nextval("));

        table.columns.push(Column {
            name,
            // The type is filled in by `introspect_column_types`, which reads the catalogue
            // fields `information_schema` does not expose.
            ty: TypeDef::Text,
            nullable: is_nullable == "YES",
            auto_increment,
            primary_key: false,
            unique: false,
            // An identity column has no default to round-trip: the sequence owns it.
            default: if auto_increment {
                None
            } else {
                default.as_deref().and_then(normalise_default)
            },
            generated: None,
            comment: None,
            renamed_from: None,
        });
    }

    introspect_column_types(db, tables, current).await
}

/// Read each column's type from `pg_catalog`.
///
/// `information_schema` collapses three things Besu has to keep apart:
///
/// - a domain is reported by its base type, so a domain over `integer` looks like a plain
///   `integer` and the next `push` would `ALTER ... TYPE` it out of existence;
/// - a type modifier is dropped, so `timestamp(3)` looks like `timestamp` and `bit(5)` like
///   `bit(1)`;
/// - an array is reported under its element type's internal name (`_int4`), which is a type
///   no one wrote.
///
/// `format_type(oid, atttypmod)` spells the whole declaration, including the modifier, and the
/// catalogue columns beside it say whether the type is a domain, an enum or an array. Nothing
/// here strips a leading underscore: an array is recognised by `typelem` and `typlen`, and
/// its element is read through the same catalogue fields.
async fn introspect_column_types<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    #[allow(clippy::type_complexity)]
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, c.relname, a.attname, \
                    format_type(a.atttypid, a.atttypmod), \
                    t.typtype::text, t.typname, tn.nspname, \
                    (t.typelem <> 0 AND t.typlen = -1), \
                    et.typtype::text, et.typname, etn.nspname, \
                    CASE WHEN a.attcollation <> 0 AND a.attcollation <> t.typcollation \
                         THEN co.collname END, \
                    bt.typtype::text, bn.nspname, bt.typname, \
                    CASE WHEN t.typbasetype <> 0 \
                         THEN format_type(t.typbasetype, t.typtypmod) END \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_type t ON t.oid = a.atttypid \
             JOIN pg_namespace tn ON tn.oid = t.typnamespace \
             LEFT JOIN pg_type et ON et.oid = t.typelem \
             LEFT JOIN pg_namespace etn ON etn.oid = et.typnamespace \
             LEFT JOIN pg_type bt ON bt.oid = NULLIF(t.typbasetype, 0) \
             LEFT JOIN pg_namespace bn ON bn.oid = bt.typnamespace \
             LEFT JOIN pg_collation co ON co.oid = a.attcollation \
             WHERE a.attnum > 0 \
               AND NOT a.attisdropped \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
               AND n.nspname NOT LIKE 'pg_temp%' \
             ORDER BY n.nspname, c.relname, a.attnum"
        ))
        .all::<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )>()
        .await?;

    for (
        schema,
        table_name,
        column_name,
        formatted,
        typtype,
        typname,
        typnamespace,
        is_array,
        element_typtype,
        element_typname,
        element_typnamespace,
        collation,
        base_typtype,
        base_typnamespace,
        base_typname,
        base_formatted,
    ) in rows
    {
        let Some(table) = tables.get_mut(&qualified(&schema, &table_name, current)) else {
            continue;
        };
        let Some(column) = table
            .columns
            .iter_mut()
            .find(|column| column.name == column_name)
        else {
            continue;
        };

        let mut ty = column_type(
            &formatted,
            &typtype,
            &typname,
            &typnamespace,
            is_array,
            element_typtype.as_deref(),
            element_typname.as_deref(),
            element_typnamespace.as_deref(),
            base_typtype.as_deref(),
            base_typname.as_deref(),
            base_typnamespace.as_deref(),
            base_formatted.as_deref(),
            current,
        );
        if let Some(collation) = collation {
            ty = TypeDef::Collated(Box::new(ty), collation.into());
        }
        column.ty = ty;
    }

    Ok(())
}

/// Turn the catalogue's description of one column into a [`TypeDef`].
#[allow(clippy::too_many_arguments)]
fn column_type(
    formatted: &str,
    typtype: &str,
    typname: &str,
    typnamespace: &str,
    is_array: bool,
    element_typtype: Option<&str>,
    element_typname: Option<&str>,
    element_typnamespace: Option<&str>,
    base_typtype: Option<&str>,
    base_typname: Option<&str>,
    base_typnamespace: Option<&str>,
    base_formatted: Option<&str>,
    current: Option<&str>,
) -> TypeDef {
    // A domain is a named type of its own, but its base type is what decides the wire format
    // a value is decoded and written through. Both are modelled, so the column is declared
    // with the domain's name and a value is read as its base type's Rust type.
    if typtype == "d" {
        let base = match (
            base_typtype,
            base_typname,
            base_typnamespace,
            base_formatted,
        ) {
            (Some(kind), Some(name), Some(namespace), Some(base)) => {
                parse_named_type(kind, name, namespace, base, current)
            }
            // The base type of a domain always exists; falling back to the domain's own name
            // keeps the column describable rather than losing it entirely.
            _ => TypeDef::Custom(qualified(typnamespace, typname, current).into()),
        };
        return TypeDef::Domain(DomainDef {
            name: qualified(typnamespace, typname, current).into(),
            base: TypeRef::owned(base),
        });
    }
    // An enum is likewise a named type; it is also listed in `Schema::enums`.
    if typtype == "e" {
        return TypeDef::Custom(qualified(typnamespace, typname, current).into());
    }
    if is_array {
        // The element's own kind decides whether it is a name that has to be qualified. The
        // `[]` suffix `format_type` writes is only used for a type Besu models directly.
        if let (Some(element), Some(element_typname), Some(element_typnamespace)) =
            (element_typtype, element_typname, element_typnamespace)
            && matches!(element, "d" | "e" | "c")
        {
            return TypeDef::Array(besu_core::types::TypeRef::owned(TypeDef::Custom(
                qualified(element_typnamespace, element_typname, current).into(),
            )));
        }
        let element = formatted.strip_suffix("[]").unwrap_or(formatted);
        return TypeDef::Array(besu_core::types::TypeRef::owned(parse_type_name(element)));
    }
    parse_type_name(formatted)
}

/// Parse a type named by the catalogue: a base type's spelling, a named type, or an array.
fn parse_named_type(
    typtype: &str,
    typname: &str,
    typnamespace: &str,
    formatted: &str,
    current: Option<&str>,
) -> TypeDef {
    match typtype {
        // An enum, a composite or another domain is a name, not a spelling. A range keeps
        // its name in `format_type` too, but the six built-in range types are recognised by
        // spelling, so `parse_type_name` sees it rather than a `Custom` in advance.
        "e" | "d" | "c" => TypeDef::Custom(qualified(typnamespace, typname, current).into()),
        _ => match formatted.strip_suffix("[]") {
            Some(element) => {
                TypeDef::Array(besu_core::types::TypeRef::owned(parse_type_name(element)))
            }
            None => parse_type_name(formatted),
        },
    }
}

/// Map a `format_type` spelling, which may carry a modifier, onto a canonical type.
fn parse_type_name(formatted: &str) -> TypeDef {
    let (base, modifier) = split_modifier(formatted);
    match base.as_str() {
        "boolean" => TypeDef::Bool,
        "smallint" => TypeDef::Int2,
        "integer" => TypeDef::Int4,
        "bigint" => TypeDef::Int8,
        "real" => TypeDef::Float4,
        "double precision" => TypeDef::Float8,
        "text" => TypeDef::Text,
        "character varying" => match number(modifier) {
            Some(length) => TypeDef::VarChar(length),
            // `VARCHAR` with no limit is `TEXT` with a different name.
            None => TypeDef::Text,
        },
        // A `CHAR` with no modifier is `CHAR(1)`, which is what PostgreSQL stores.
        "character" => TypeDef::Char(number(modifier).unwrap_or(1)),
        "bytea" => TypeDef::Bytes,
        "uuid" => TypeDef::Uuid,
        "json" => TypeDef::Json,
        "jsonb" => TypeDef::Jsonb,
        "date" => TypeDef::Date,
        "time without time zone" => match number(modifier) {
            Some(precision) => TypeDef::TimeP(narrow(precision)),
            None => TypeDef::Time,
        },
        "time with time zone" => match number(modifier) {
            Some(precision) => TypeDef::TimeTzP(narrow(precision)),
            None => TypeDef::TimeTz,
        },
        "timestamp without time zone" => match number(modifier) {
            Some(precision) => TypeDef::TimestampP(narrow(precision)),
            None => TypeDef::Timestamp,
        },
        "timestamp with time zone" => match number(modifier) {
            Some(precision) => TypeDef::TimestampTzP(narrow(precision)),
            None => TypeDef::TimestampTz,
        },
        "numeric" | "decimal" => match modifier.and_then(decimal_parts) {
            Some((precision, scale)) => TypeDef::Decimal(precision, scale),
            // A `NUMERIC` with no modifier has no fixed precision, which `Decimal` cannot
            // express; its name is kept and DDL names it again.
            None => TypeDef::Custom("numeric".into()),
        },
        "bit" => TypeDef::Bit(number(modifier).unwrap_or(1)),
        // `BIT VARYING` with no modifier is unlimited, which `VarBit(0)` records.
        "bit varying" => TypeDef::VarBit(number(modifier).unwrap_or(0)),
        "interval" => TypeDef::Interval,
        "inet" => TypeDef::Inet,
        "cidr" => TypeDef::Cidr,
        // The six built-in range types. Their subtype is what the value's wire format is, and
        // it is also the only thing the type name encodes.
        "int4range" => TypeDef::Range(TypeRef::from_static(&TypeDef::Int4)),
        "int8range" => TypeDef::Range(TypeRef::from_static(&TypeDef::Int8)),
        // `numrange`'s subtype is an unconstrained `numeric`, which `Decimal` cannot express;
        // `Decimal(0, 0)` is the spelling the dialect folds to and renders from.
        "numrange" => TypeDef::Range(TypeRef::from_static(&TypeDef::Decimal(0, 0))),
        "tsrange" => TypeDef::Range(TypeRef::from_static(&TypeDef::Timestamp)),
        "tstzrange" => TypeDef::Range(TypeRef::from_static(&TypeDef::TimestampTz)),
        "daterange" => TypeDef::Range(TypeRef::from_static(&TypeDef::Date)),
        // PostGIS spells a narrowed column `geometry(Point,4326)`, which `split_modifier` has
        // already separated into the base name and the `Point,4326` modifier.
        "geometry" => spatial_type(false, modifier),
        "geography" => spatial_type(true, modifier),
        // `pgvector` reports `vector(3)` and, for a column declared without a dimension,
        // `vector`.
        "vector" => TypeDef::Vector(number(modifier).unwrap_or(0)),
        // An enum, a composite, a range, `tsvector` and every extension type keep their
        // native spelling, so generated DDL names the same type.
        _ => TypeDef::Custom(formatted.to_owned().into()),
    }
}

/// Build a PostGIS `geometry`/`geography` type from `format_type`'s modifier.
///
/// A plain `geometry` has no modifier at all; a narrowed one spells the shape and,
/// optionally, the SRID — `Point,4326`.
fn spatial_type(geography: bool, modifier: Option<&str>) -> TypeDef {
    use besu_core::GeometryKind;

    let (kind, srid) = match modifier {
        None => (GeometryKind::Geometry, None),
        Some(modifier) => {
            let mut parts = modifier.split(',');
            let kind = parts
                .next()
                .map(GeometryKind::from_name)
                .unwrap_or(GeometryKind::Geometry);
            let srid = parts.next().and_then(|srid| srid.trim().parse().ok());
            (kind, srid)
        }
    };
    if geography {
        TypeDef::Geography { kind, srid }
    } else {
        TypeDef::Geometry { kind, srid }
    }
}

/// Split `timestamp(3) without time zone` into `timestamp without time zone` and `3`.
fn split_modifier(formatted: &str) -> (String, Option<&str>) {
    let Some(open) = formatted.find('(') else {
        return (formatted.to_owned(), None);
    };
    let Some(close) = formatted[open..].find(')').map(|close| close + open) else {
        return (formatted.to_owned(), None);
    };
    let base = format!("{}{}", &formatted[..open], &formatted[close + 1..]);
    (base.trim().to_owned(), Some(&formatted[open + 1..close]))
}

/// Narrow a parsed precision to the `u8` a [`TypeDef`] carries.
fn narrow(precision: u32) -> u8 {
    u8::try_from(precision).unwrap_or(u8::MAX)
}

/// The single number in a modifier such as `3`.
fn number(modifier: Option<&str>) -> Option<u32> {
    modifier?.trim().parse().ok()
}

/// The `(precision, scale)` in a `numeric` modifier such as `10,2`.
fn decimal_parts(modifier: &str) -> Option<(u8, u8)> {
    let mut parts = modifier.split(',');
    let precision = parts.next()?.trim().parse().ok()?;
    let scale = parts
        .next()
        .map_or(0, |part| part.trim().parse().unwrap_or_default());
    Some((precision, scale))
}

/// Read the expression of every generated column.
///
/// This is a query of its own because the column row already has twelve fields and the
/// driver decodes tuples up to twelve; `information_schema.columns.is_generated` is the
/// catalogue's own marker, so a plain column is never mistaken for one.
async fn introspect_generated_columns<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT table_schema, table_name, column_name, generation_expression \
             FROM information_schema.columns \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND table_schema NOT LIKE 'pg_toast%' \
               AND is_generated = 'ALWAYS' \
             ORDER BY table_schema, table_name, ordinal_position"
        ))
        .all::<(String, String, String, Option<String>)>()
        .await?;

    for (schema, table_name, column_name, expression) in rows {
        // A column whose expression could not be read is left as a plain column: reporting
        // it as generated with no way to recreate it would make every push drop and add it.
        let Some(expression) = expression.filter(|expression| !expression.trim().is_empty()) else {
            continue;
        };
        let Some(table) = tables.get_mut(&qualified(&schema, &table_name, current)) else {
            continue;
        };
        let Some(column) = table
            .columns
            .iter_mut()
            .find(|column| column.name == column_name)
        else {
            continue;
        };
        column.generated = Some(Generated {
            expression: normalise_generated_expression(&expression),
            // PostgreSQL implements stored generated columns only.
            stored: true,
        });
        // A generated column owns its value, exactly as an identity column does; the flag
        // would make the DDL render both spellings.
        column.auto_increment = false;
    }

    Ok(())
}

/// Read primary keys and `UNIQUE` constraints, and record their backing indexes so those
/// indexes are not reported as ordinary ones.
///
/// A single-column `UNIQUE` is folded onto its column, which is how the derive declares it.
/// A composite one becomes [`Table::uniques`], a constraint the database owns: reporting it
/// as an index is what made `DROP INDEX` fail on it.
///
/// `constraint_type IN (...)` deliberately leaves out `EXCLUDE`: an exclusion constraint is
/// not modelled at all, so it is skipped rather than folded into a `Unique` that is not
/// there. PostgreSQL 17 also lists `NOT NULL` as a `CHECK` here, but without a
/// `key_column_usage` row, which is why the inner join drops it.
async fn introspect_constraints<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT tc.table_schema, tc.table_name, tc.constraint_name, tc.constraint_type, \
                    kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_schema = tc.constraint_schema \
              AND kcu.constraint_name = tc.constraint_name \
              AND kcu.table_schema = tc.table_schema \
              AND kcu.table_name = tc.table_name \
             WHERE tc.table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND tc.table_schema NOT LIKE 'pg_toast%' \
               AND tc.constraint_type IN ('PRIMARY KEY', 'UNIQUE') \
             ORDER BY tc.table_schema, tc.table_name, tc.constraint_name, kcu.ordinal_position"
        ))
        .all::<(String, String, String, String, String)>()
        .await?;

    struct Constraint {
        table: String,
        name: String,
        kind: String,
        columns: Vec<String>,
    }

    let mut constraints: Vec<Constraint> = Vec::new();
    for (schema, table, name, kind, column) in rows {
        let table = qualified(&schema, &table, current);
        match constraints
            .iter_mut()
            .find(|constraint| constraint.table == table && constraint.name == name)
        {
            Some(constraint) => constraint.columns.push(column),
            None => constraints.push(Constraint {
                table,
                name,
                kind,
                columns: vec![column],
            }),
        }
    }

    for constraint in constraints {
        let Some(table) = tables.get_mut(&constraint.table) else {
            continue;
        };
        if constraint.kind == "PRIMARY KEY" {
            for name in &constraint.columns {
                if let Some(column) = table.columns.iter_mut().find(|column| &column.name == name) {
                    column.primary_key = true;
                }
            }
        } else if let [column_name] = constraint.columns.as_slice() {
            // A single-column `UNIQUE` is also reported on the column, which is how the
            // derive declares it, so it round-trips instead of becoming an index.
            if let Some(column) = table
                .columns
                .iter_mut()
                .find(|column| &column.name == column_name)
            {
                column.unique = true;
            }
        } else {
            // A composite `UNIQUE` has no per-column spelling. PostgreSQL owns a backing
            // index through the constraint, and `DROP INDEX` cannot remove it, so it is a
            // constraint in its own right.
            table.uniques.push(Unique {
                name: constraint.name,
                columns: constraint.columns,
            });
        }
    }

    Ok(())
}

/// Read `CHECK` constraints from `pg_constraint`.
///
/// `contype = 'c'` is only a real `CHECK` a person wrote: `NOT NULL` has no row here, and a
/// domain's constraint belongs to the type, not the table. `conislocal` excludes a check a
/// table merely inherited from its parent, which the table itself cannot drop. PostgreSQL
/// stores the expression with extra parentheses (`CHECK (value > 0)` comes back as
/// `(value > 0)`), which [`normalise_expression`] folds away so the declared spelling
/// converges.
async fn introspect_checks<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, t.relname, con.conname, pg_get_expr(con.conbin, con.conrelid) \
             FROM pg_constraint con \
             JOIN pg_class t ON t.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE con.contype = 'c' \
               AND con.conislocal \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
               AND n.nspname NOT LIKE 'pg_temp%' \
             ORDER BY n.nspname, t.relname, con.conname"
        ))
        .all::<(String, String, String, Option<String>)>()
        .await?;

    for (schema, name, constraint, expression) in rows {
        // A check with no readable expression cannot be compared to anything, so reporting
        // it would make every push drop and re-add it.
        let Some(expression) = expression else {
            continue;
        };
        let Some(table) = tables.get_mut(&qualified(&schema, &name, current)) else {
            continue;
        };
        table.checks.push(Check {
            name: constraint,
            expression: normalise_expression(&expression),
        });
    }

    Ok(())
}

async fn introspect_indexes<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, t.relname, i.relname, ix.indisunique, am.amname, \
                    ix.indnkeyatts, ix.indnatts, a.attname, \
                    pg_get_indexdef(ix.indexrelid, k.ord::int, true), \
                    opc.opcname, opc.opcdefault, \
                    pg_get_expr(ix.indpred, ix.indrelid) \
             FROM pg_index ix \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_am am ON am.oid = i.relam \
             JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS k(attnum, ord) \
               ON k.ord <= ix.indnkeyatts \
             JOIN LATERAL unnest(ix.indclass) WITH ORDINALITY AS class(opcoid, ord) \
               ON class.ord = k.ord \
             JOIN pg_opclass opc ON opc.oid = class.opcoid \
             LEFT JOIN pg_attribute a \
               ON a.attrelid = t.oid AND a.attnum = k.attnum AND k.attnum <> 0 \
             WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
               AND NOT ix.indisprimary \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_constraint con WHERE con.conindid = ix.indexrelid \
               ) \
             ORDER BY n.nspname, t.relname, i.relname, k.ord"
        ))
        .all::<(
            String,
            String,
            String,
            bool,
            String,
            i16,
            i16,
            Option<String>,
            String,
            String,
            bool,
            Option<String>,
        )>()
        .await?;

    struct PgIndex {
        table: String,
        name: String,
        unique: bool,
        kind: IndexKind,
        keys: usize,
        attributes: usize,
        columns: Vec<IndexColumn>,
        predicate: Option<String>,
    }

    let mut indexes: Vec<PgIndex> = Vec::new();
    for (
        schema,
        table,
        name,
        unique,
        method,
        keys,
        attributes,
        column,
        key_definition,
        opclass,
        default_opclass,
        predicate,
    ) in rows
    {
        let Some(kind) = parse_access_method(&method) else {
            continue;
        };
        let table = qualified(&schema, &table, current);
        // `indkey` names the column when the key part is one, and is zero for an
        // expression; `pg_get_indexdef` returns the expression text for that ordinal. The
        // operator class is separate, and only worth recording when it is not the type's
        // default: otherwise every ordinary index would carry `text_ops` and never converge
        // against a declaration that does not name one.
        let entry = match column {
            Some(column) => IndexColumn::column(column),
            None => IndexColumn::expression(key_definition),
        };
        let entry = if default_opclass {
            entry
        } else {
            entry.with_opclass(opclass)
        };
        match indexes
            .iter_mut()
            .find(|index| index.table == table && index.name == name)
        {
            Some(index) => index.columns.push(entry),
            None => indexes.push(PgIndex {
                table,
                name,
                unique,
                kind,
                keys: usize::try_from(keys).unwrap_or(0),
                attributes: usize::try_from(attributes).unwrap_or(0),
                columns: vec![entry],
                predicate: predicate.map(|predicate| normalise_expression(&predicate)),
            }),
        }
    }

    for index in indexes {
        // A non-key `INCLUDE` column is not a key column, and the IR cannot name one. An
        // index with `INCLUDE` would otherwise be reported as a plain index over its key
        // columns only, which is not the index that exists; skip it instead. Indexes that
        // back a constraint — a primary key, a `UNIQUE`, or an exclusion constraint, which
        // has no representation at all — are excluded by the query, because `DROP INDEX`
        // cannot remove them.
        if index.attributes != index.keys {
            continue;
        }

        let Some(table) = tables.get_mut(&index.table) else {
            continue;
        };
        table.indexes.push(Index {
            name: index.name,
            columns: index.columns,
            unique: index.unique,
            kind: index.kind,
            predicate: index.predicate,
        });
    }

    Ok(())
}

/// Map `pg_am.amname` onto an [`IndexKind`].
///
/// `None` means the IR has no name for the method (`hash`, `spgist`), so the index is
/// skipped rather than reported as a b-tree that is not there.
fn parse_access_method(method: &str) -> Option<IndexKind> {
    match method {
        "btree" => Some(IndexKind::Btree),
        "gin" => Some(IndexKind::Gin),
        "gist" => Some(IndexKind::Gist),
        "brin" => Some(IndexKind::Brin),
        _ => None,
    }
}

async fn introspect_foreign_keys<Dr>(
    db: &besu::Database<Dr>,
    tables: &mut BTreeMap<String, Table>,
    current: Option<&str>,
) -> Result<(), Dr::Error>
where
    Dr: besu::Driver<Dialect = Postgres>,
    String: besu_core::Decode<Dr>,
    i64: besu_core::Decode<Dr>,
    bool: besu_core::Decode<Dr>,
    i16: besu_core::Decode<Dr>,
{
    let rows = db
        .execute(besu::sql!(
            "SELECT n.nspname, t.relname, con.conname, rn.nspname, rt.relname, \
                    con.confdeltype::text, con.confupdtype::text, a.attname, ra.attname \
             FROM pg_constraint con \
             JOIN pg_class t ON t.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_class rt ON rt.oid = con.confrelid \
             JOIN pg_namespace rn ON rn.oid = rt.relnamespace \
             JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) ON true \
             JOIN LATERAL unnest(con.confkey) WITH ORDINALITY AS rk(attnum, ord) \
               ON rk.ord = k.ord \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum \
             JOIN pg_attribute ra ON ra.attrelid = rt.oid AND ra.attnum = rk.attnum \
             WHERE con.contype = 'f' \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg_toast%' \
             ORDER BY n.nspname, t.relname, con.conname, k.ord"
        ))
        .all::<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        )>()
        .await?;

    let mut foreign_keys: Vec<(String, ForeignKey)> = Vec::new();
    for (
        schema,
        table,
        name,
        references_schema,
        references_table,
        on_delete,
        on_update,
        column,
        references_column,
    ) in rows
    {
        let table = qualified(&schema, &table, current);
        // The referenced table is qualified the same way as any other name, so a constraint
        // that crosses schemas reaches the right table.
        let references_table = qualified(&references_schema, &references_table, current);

        if let Some((_, fk)) = foreign_keys
            .iter_mut()
            .find(|(existing, fk)| existing == &table && fk.name == name)
        {
            fk.columns.push(column);
            fk.references_columns.push(references_column);
            continue;
        }

        foreign_keys.push((
            table,
            ForeignKey {
                name,
                columns: vec![column],
                references_table,
                references_columns: vec![references_column],
                on_delete: parse_action(&on_delete),
                on_update: parse_action(&on_update),
            },
        ));
    }

    for (table, fk) in foreign_keys {
        if let Some(table) = tables.get_mut(&table) {
            table.foreign_keys.push(fk);
        }
    }

    Ok(())
}

/// Map PostgreSQL's `confdeltype` / `confupdtype` codes onto an action.
///
/// `a` is `NO ACTION`, which is also what PostgreSQL reports when no action was written.
/// [`ReferenceAction::Default`] renders as no clause at all, so it round-trips to `NO ACTION`.
fn parse_action(code: &str) -> ReferenceAction {
    match code {
        "c" => ReferenceAction::Cascade,
        "n" => ReferenceAction::SetNull,
        "d" => ReferenceAction::SetDefault,
        "r" => ReferenceAction::Restrict,
        _ => ReferenceAction::Default,
    }
}

/// Normalise a `column_default` into the expression the derive would produce.
///
/// PostgreSQL stores defaults fully typed, as in `'oscar'::text` or `(0)::numeric`, so the
/// casts and wrapping parentheses are stripped to leave `'oscar'` and `0`.
fn normalise_default(value: &str) -> Option<String> {
    let mut normalised = value.trim().to_owned();

    while let Some(index) = trailing_cast(&normalised) {
        normalised.truncate(index);
        normalised = normalised.trim().to_owned();
    }

    loop {
        let trimmed = normalised.trim();
        match strip_parens(trimmed) {
            Some(inner) => normalised = inner.trim().to_owned(),
            None => {
                normalised = trimmed.to_owned();
                break;
            }
        }
    }

    if normalised.is_empty() {
        None
    } else {
        Some(normalised)
    }
}

/// The byte offset of a trailing `::type` cast, if there is one outside quotes and parentheses.
fn trailing_cast(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut quote = 0u8;
    let mut cast = None;
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if quote != 0 {
            if byte == quote {
                if bytes.get(index + 1) == Some(&quote) {
                    index += 1;
                } else {
                    quote = 0;
                }
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = byte,
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b':' if depth == 0 && bytes.get(index + 1) == Some(&b':') => {
                    cast = Some(index);
                    index += 1;
                }
                _ => {}
            }
        }
        index += 1;
    }

    let index = cast?;
    let suffix = &sql[index + 2..];
    let looks_like_type = !suffix.is_empty()
        && suffix.chars().all(|ch| {
            ch.is_alphanumeric() || matches!(ch, ' ' | '(' | ')' | '[' | ']' | '_' | '.' | '"')
        });
    looks_like_type.then_some(index)
}

/// Strip one pair of parentheses that wraps the whole expression.
fn strip_parens(sql: &str) -> Option<&str> {
    let inner = sql.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    let mut quote = 0u8;
    for byte in inner.bytes() {
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = byte,
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                if depth == 0 {
                    // The opening parenthesis closes before the end, so the outer pair does
                    // not wrap the whole expression.
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    Some(inner)
}
