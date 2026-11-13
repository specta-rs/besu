use besu_core::{Dialect, TypeDef};
use besu_schema::{
    Change, Column, Ddl, Index, IndexColumn, IndexKind, IndexTarget, Schema, Table, View, view_body,
};

use crate::MySql;

/// DDL generation for MySQL.
#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlDdl;

/// The prefix length used when the index has to cover part of a `TEXT` or `BLOB` column.
///
/// MySQL cannot index those types without a prefix. 255 characters is under the 3072-byte
/// key limit for `utf8mb4` with InnoDB's default row format, and is what MySQL itself
/// suggests for a unique short-string key.
const INDEX_PREFIX: u32 = 255;

/// The widest key, in bytes, InnoDB accepts.
const MAX_KEY_BYTES: usize = 3072;

/// How many `utf8mb4` characters fit in [`MAX_KEY_BYTES`].
///
/// A `VARCHAR(1000)` is four bytes per character at worst, so a full key is impossible; the
/// prefix keeps the most of the value that fits.
const MAX_PREFIX_CHARS: u32 = (MAX_KEY_BYTES / 4) as u32;

/// How a `UNIQUE` constraint can be attached to a column of a given type.
enum Uniqueness {
    /// The type is fully indexable, so `UNIQUE` can be written on the column itself.
    Inline,
    /// The type needs an index prefix, so uniqueness becomes a separate key.
    Prefix(u32),
    /// The type cannot be indexed at all (`JSON`), so a `UNIQUE` on it is written as
    /// nothing. MySQL offers no way to enforce it and `Ddl` has no error channel, so such a
    /// schema will keep reporting drift on every push.
    Unsupported,
}

/// The type under any collation, which never changes how a column is indexed.
fn bare_type(ty: &TypeDef) -> &TypeDef {
    match ty {
        TypeDef::Collated(inner, _) => bare_type(inner),
        other => other,
    }
}

fn uniqueness(ty: &TypeDef) -> Uniqueness {
    match bare_type(ty) {
        TypeDef::Text | TypeDef::Bytes => Uniqueness::Prefix(INDEX_PREFIX),
        // A string type is indexable only while its worst-case byte length fits. `utf8mb4`
        // uses up to four bytes per character, so anything wider than 768 characters needs
        // a prefix or InnoDB rejects the key with "Specified key was too long".
        TypeDef::VarChar(length) | TypeDef::Char(length) => {
            let characters = usize::try_from(*length).unwrap_or(usize::MAX);
            if characters.saturating_mul(4) > MAX_KEY_BYTES {
                Uniqueness::Prefix(MAX_PREFIX_CHARS.min(*length))
            } else {
                Uniqueness::Inline
            }
        }
        TypeDef::Json | TypeDef::Jsonb => Uniqueness::Unsupported,
        _ => Uniqueness::Inline,
    }
}

impl MySqlDdl {
    /// Whether a default for this type has to be written as a parenthesised expression.
    ///
    /// MySQL 8 only accepts a literal default on `TEXT`, `BLOB` and `JSON` if it is
    /// spelled as an expression, because those types did not support defaults at all
    /// before 8.0.13.
    fn default_needs_expression(ty: &TypeDef) -> bool {
        matches!(
            bare_type(ty),
            TypeDef::Text | TypeDef::Bytes | TypeDef::Json | TypeDef::Jsonb
        )
    }

    /// Render a key-part list with the key lengths and functional spellings MySQL needs.
    ///
    /// `create_index` only sees the index, and an `AddIndex` change is rendered from a
    /// `CREATE INDEX` statement that cannot look the column up on its own; both paths go
    /// through here so a `TEXT` or oversized `VARCHAR` column gets the same prefix either
    /// way. Without it, adding an index to an existing `String` column fails with error 1170.
    ///
    /// MySQL 8 spells a functional key part as `(expression)`, with the parentheses part of
    /// the key-part syntax rather than the surrounding key list. Operator classes do not
    /// exist here, so they are omitted.
    fn key_columns(&self, table: Option<&Table>, columns: &[IndexColumn]) -> String {
        let mut out = String::new();
        for (i, entry) in columns.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            match &entry.target {
                IndexTarget::Column(name) => {
                    out.push_str(&self.quote(name));
                    if let Some(column) = table.and_then(|table| table.column(name))
                        && let Uniqueness::Prefix(prefix) = uniqueness(&column.ty)
                    {
                        out.push_str(&format!("({prefix})"));
                    }
                }
                IndexTarget::Expression(expression) => {
                    out.push('(');
                    out.push_str(expression);
                    out.push(')');
                }
            }
        }
        out
    }

    /// `CREATE [FULLTEXT|SPATIAL] INDEX`, which knows the table's column types.
    fn add_index(&self, table: &str, index: &Index, definition: Option<&Table>) -> Vec<String> {
        let name = self.quote(&index.name);
        let table = self.quote_table(table);
        let columns = self.key_columns(definition, &index.columns);
        match index.kind {
            // A full-text or spatial index takes no prefix and no `UNIQUE`: MySQL indexes
            // the whole value and has no unique full-text index.
            IndexKind::FullText => vec![format!(
                "CREATE FULLTEXT INDEX {name} ON {table} ({})",
                self.key_columns(None, &index.columns)
            )],
            IndexKind::Spatial => vec![format!(
                "CREATE SPATIAL INDEX {name} ON {table} ({})",
                self.key_columns(None, &index.columns)
            )],
            // MySQL has no GIN, GiST or BRIN. The closest thing it has is a plain key, and
            // reporting the drift on every push is better than dropping the column's index.
            _ => {
                let unique = if index.unique { "UNIQUE " } else { "" };
                vec![format!(
                    "CREATE {unique}INDEX {name} ON {table} ({columns})"
                )]
            }
        }
    }
}

impl Ddl for MySqlDdl {
    fn name(&self) -> &'static str {
        "mysql"
    }

    // MySQL needs a prefix length to index a `TEXT` or `BLOB` column, which `create_index`
    // cannot know without the table, so the indexes are rendered inside `CREATE TABLE`.
    const INLINE_INDEXES: bool = true;

    /// An enum is spelled inside the column, so there is no type object for `push` to
    /// create or drop.
    fn inline_enums(&self) -> bool {
        true
    }

    fn quote(&self, ident: &str) -> String {
        MySql::quote(ident)
    }

    fn type_name(&self, ty: &TypeDef) -> String {
        MySql::type_name(ty.clone()).into_owned()
    }

    fn normalise_type(&self, ty: &TypeDef) -> TypeDef {
        // MySQL stores a `TIMETZ` as a plain `TIME` and drops the offset, so a schema
        // declaring `TIMETZ` must compare equal to the `TIME` the database reports back.
        match ty {
            TypeDef::TimeTz => TypeDef::Time,
            // A precision is part of the type, and `TIME(6)` is exactly what the declared
            // `Time` renders, so folding the two keeps `push` a no-op. Any other precision
            // is a different column and keeps its modifier.
            TypeDef::TimeTzP(6) | TypeDef::TimeP(6) => TypeDef::Time,
            TypeDef::TimeTzP(p) => TypeDef::TimeP(*p),
            // MySQL has no column type that stores an offset: `DATETIME` is the same column
            // for both, so a declared `TimestampTz` must compare equal to the `Timestamp`
            // introspection reports — otherwise every `push` sees a type change.
            TypeDef::TimestampTz | TypeDef::TimestampP(6) | TypeDef::TimestampTzP(6) => {
                TypeDef::Timestamp
            }
            TypeDef::TimestampTzP(p) => TypeDef::TimestampP(*p),
            // `u128` is `DECIMAL(39,0)`, which is indistinguishable from a
            // `Decimal(39, 0)`; an `i128` is declared as that already.
            TypeDef::UInt(128) => TypeDef::Decimal(39, 0),
            // MySQL has no array type; the declared DDL falls back to `TEXT`, so a declared
            // array has to fold onto the text column introspection reports.
            TypeDef::Array(_) => TypeDef::Text,
            // MySQL has no interval, address or bit-string text type either: all three are
            // declared `TEXT` and the canonical text is what comes back.
            TypeDef::Interval | TypeDef::Inet | TypeDef::Cidr => TypeDef::Text,
            // MySQL has one bit type. `VARBIT(n)` is declared `BIT(n)`, and an unbounded
            // `VARBIT` is `BIT(64)`, so the two fold together by width.
            TypeDef::VarBit(0) => TypeDef::Bit(64),
            TypeDef::VarBit(n) => TypeDef::Bit(*n),
            // MySQL has no domain: the column is the base type it was built on.
            TypeDef::Domain(def) => self.normalise_type(def.base.as_type()),
            // An enum is inline, so its name names nothing and is dropped: the labels are
            // the whole type. Introspection reports the labels with no name, and folding
            // the name away is what makes the declared column equal to the one the database
            // reports back. A changed label set is still a different type, and therefore an
            // `AlterColumn` rather than a table rebuild.
            TypeDef::Enum(def) => TypeDef::Enum(besu_core::types::EnumDef {
                name: std::borrow::Cow::Borrowed(""),
                labels: def.labels.clone(),
            }),
            // A collation does not change the type under it, so the fold is applied to the
            // inner type and the name is kept.
            TypeDef::Collated(inner, collation) => {
                TypeDef::Collated(Box::new(self.normalise_type(inner)), collation.clone())
            }
            // MySQL has finer storage widths than the canonical types: a signed `TINYINT`
            // is narrower than `SMALLINT`, and `LONGTEXT` holds more than `TEXT`. Their
            // native names are kept for fidelity, and folded here so a declared `i8` or
            // `String` converges against the column the database reports.
            TypeDef::Custom(name) => {
                let lower = name.to_ascii_lowercase();
                // An older server writes `tinyint(4)`, so the display width is ignored.
                match lower.split('(').next().unwrap_or(lower.as_str()) {
                    "tinyint" | "year" => TypeDef::Int2,
                    "mediumint" => TypeDef::Int4,
                    "tinytext" | "mediumtext" | "longtext" => TypeDef::Text,
                    "tinyblob" | "mediumblob" | "longblob" => TypeDef::Bytes,
                    _ => TypeDef::Custom(name.clone()),
                }
            }
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
            // MySQL implements both spellings. A stored column occupies space and can be
            // indexed; a virtual one is computed when read.
            sql.push_str(" GENERATED ALWAYS AS (");
            sql.push_str(&generated.expression);
            sql.push_str(if generated.stored {
                ") STORED"
            } else {
                ") VIRTUAL"
            });
        }

        // MySQL makes primary key columns `NOT NULL` implicitly, but spelling it out keeps
        // the generated `CREATE TABLE` readable and matches `SHOW CREATE TABLE`.
        if !column.nullable && (!column.primary_key || column.auto_increment) {
            sql.push_str(" NOT NULL");
        }

        // `AUTO_INCREMENT` requires the column to be a key, which the inline `PRIMARY KEY`
        // below provides. A generated column owns its value, so it can never have one.
        if column.auto_increment && column.generated.is_none() {
            sql.push_str(" AUTO_INCREMENT");
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        if column.unique
            && column.generated.is_none()
            && matches!(uniqueness(&column.ty), Uniqueness::Inline)
        {
            sql.push_str(" UNIQUE");
        }

        // A generated column is computed from its expression and MySQL rejects a default
        // beside one.
        if column.generated.is_none()
            && let Some(default) = &column.default
        {
            sql.push_str(" DEFAULT ");
            // An `ON UPDATE` clause is never an expression, even on a type that otherwise
            // needs the parenthesised form.
            if Self::default_needs_expression(&column.ty) && !default.contains(" ON UPDATE ") {
                sql.push('(');
                sql.push_str(default);
                sql.push(')');
            } else {
                sql.push_str(default);
            }
        }

        sql
    }

    fn alter_column(&self, table: &str, from: &Column, to: &Column) -> Vec<String> {
        // Changing a generated column's expression, or turning a plain column into a
        // generated one, is not something `MODIFY COLUMN` promises to do (the stored/virtual
        // kind in particular cannot change). A generated column holds no data of its own, so
        // dropping and adding is lossless.
        if from.generated != to.generated {
            return vec![
                self.drop_column(table, &from.name),
                self.add_column(table, to),
            ];
        }

        // MySQL has no `ALTER COLUMN ... TYPE`: `MODIFY COLUMN` replaces the whole
        // definition, so the new column is rendered exactly as it would be in
        // `CREATE TABLE`. `inline_pk` is false because indexes (including the primary key)
        // survive a `MODIFY COLUMN` on their own.
        vec![format!(
            "ALTER TABLE {} MODIFY COLUMN {}",
            self.quote_table(table),
            self.column_sql(to, false)
        )]
    }

    fn drop_index(&self, table: &str, index: &besu_schema::Index) -> String {
        format!(
            "DROP INDEX {} ON {}",
            self.quote(&index.name),
            self.quote_table(table)
        )
    }

    fn drop_foreign_key(&self, table: &str, fk: &besu_schema::ForeignKey) -> String {
        // MySQL drops a foreign key by constraint name, without the `CONSTRAINT` keyword.
        format!(
            "ALTER TABLE {} DROP FOREIGN KEY {}",
            self.quote_table(table),
            self.quote(&fk.name)
        )
    }

    fn drop_check(&self, table: &str, check: &besu_schema::Check) -> String {
        // MySQL 8.0.16 spells the drop `DROP CHECK`; `DROP CONSTRAINT` only arrived in
        // 8.0.19, so this is the spelling that works on every server that enforces checks.
        format!(
            "ALTER TABLE {} DROP CHECK {}",
            self.quote_table(table),
            self.quote(&check.name)
        )
    }

    /// `CREATE TABLE`.
    ///
    /// Secondary indexes are inlined as `KEY` clauses because `create_schema` only runs
    /// `CREATE TABLE`, and only the table knows its column types — MySQL needs an explicit
    /// key length to index a `TEXT` or `BLOB` column. The consequence is that an
    /// `AddIndex` change on such a column (`CREATE INDEX`, which never sees the types) has
    /// to be written by hand.
    fn create_table(&self, table: &Table) -> String {
        let mut sql = String::from("CREATE TABLE ");
        sql.push_str(&self.quote_table(&table.qualified_name()));
        sql.push_str(" (");

        let primary_key = table.primary_key();
        let inline_pk = primary_key.len() == 1;

        for (i, column) in table.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(&self.column_sql(column, inline_pk));
        }

        if primary_key.len() > 1 {
            sql.push_str(", PRIMARY KEY (");
            for (i, column) in primary_key.iter().enumerate() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&self.quote(&column.name));
            }
            sql.push(')');
        }

        // `TEXT` and `BLOB` columns cannot carry an inline `UNIQUE`, so their uniqueness is
        // declared as a prefix key instead.
        for column in &table.columns {
            if column.unique {
                if let Uniqueness::Prefix(prefix) = uniqueness(&column.ty) {
                    sql.push_str(", UNIQUE KEY ");
                    sql.push_str(&self.quote(&column.name));
                    sql.push_str(" (");
                    sql.push_str(&self.quote(&column.name));
                    sql.push_str(&format!("({prefix}))"));
                }
            }
        }

        // Composite `UNIQUE` constraints. MySQL has no object distinct from the unique key
        // that enforces them, so they are written as a named key.
        for unique in &table.uniques {
            sql.push_str(", ");
            sql.push_str(&self.unique_clause(unique));
        }

        // `CHECK` constraints. MySQL 8.0.16 and later enforce them; an older server parses
        // and ignores the clause.
        for check in &table.checks {
            sql.push_str(", ");
            sql.push_str(&self.check_clause(check));
        }

        // Secondary indexes are inlined here because `create_schema` only runs
        // `CREATE TABLE`, and MySQL has no `CREATE INDEX` clause inside `ALTER TABLE`.
        // MySQL has no partial indexes, so `Index::predicate` cannot be rendered and is
        // ignored; introspection never produces one.
        for index in &table.indexes {
            sql.push_str(", ");
            let name = self.quote(&index.name);
            match index.kind {
                // `FULLTEXT` and `SPATIAL` take no prefix and no `UNIQUE`.
                IndexKind::FullText => {
                    sql.push_str("FULLTEXT KEY ");
                    sql.push_str(&name);
                    sql.push_str(" (");
                    sql.push_str(&self.key_columns(None, &index.columns));
                    sql.push(')');
                }
                IndexKind::Spatial => {
                    sql.push_str("SPATIAL KEY ");
                    sql.push_str(&name);
                    sql.push_str(" (");
                    sql.push_str(&self.key_columns(None, &index.columns));
                    sql.push(')');
                }
                _ => {
                    if index.unique {
                        sql.push_str("UNIQUE ");
                    }
                    sql.push_str("KEY ");
                    sql.push_str(&name);
                    sql.push_str(" (");
                    sql.push_str(&self.key_columns(Some(table), &index.columns));
                    sql.push(')');
                }
            }
        }

        for fk in &table.foreign_keys {
            sql.push_str(", ");
            sql.push_str(&self.foreign_key_clause(fk));
        }

        // InnoDB for transactions and foreign keys, `utf8mb4` because it is the only
        // character set that can store every Unicode character (including emoji).
        sql.push_str(") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci");
        sql
    }

    /// `CREATE [FULLTEXT|SPATIAL] INDEX`.
    ///
    /// This spelling is used when the table's column types are not in reach. The
    /// `AddIndex` path goes through [`Ddl::apply_with`], which looks the table up and
    /// renders the prefixes a `TEXT` or oversized `VARCHAR` column needs.
    fn create_index(&self, table: &str, index: &Index) -> String {
        self.add_index(table, index, None)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// `CREATE [OR REPLACE] VIEW`.
    ///
    /// MySQL has no materialised views. A snapshot that declares one is created as an
    /// ordinary view, which is documented drift: introspection reports it as plain.
    fn create_view(&self, view: &View) -> Vec<String> {
        vec![format!(
            "CREATE VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// `CREATE OR REPLACE VIEW`.
    fn replace_view(&self, view: &View) -> Vec<String> {
        vec![format!(
            "CREATE OR REPLACE VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// `DROP VIEW`.
    ///
    /// MySQL has no materialised views, so the flag is ignored and the statement it accepts
    /// is always a plain drop.
    fn drop_view(&self, name: &str, _materialised: bool) -> Vec<String> {
        vec![format!("DROP VIEW {}", self.quote_table(name))]
    }

    fn change(&self, change: &Change) -> Vec<String> {
        match change {
            // `ALTER TABLE ... ADD COLUMN` cannot add an index, so a unique column whose
            // type needs a prefix is followed by a separate `CREATE UNIQUE INDEX`.
            Change::AddColumn { table, column } => {
                let mut statements = vec![self.add_column(table, column)];
                if column.unique && column.generated.is_none() {
                    if let Uniqueness::Prefix(prefix) = uniqueness(&column.ty) {
                        statements.push(format!(
                            "CREATE UNIQUE INDEX {} ON {} ({}({}))",
                            self.quote(&column.name),
                            self.quote_table(table),
                            self.quote(&column.name),
                            prefix
                        ));
                    }
                }
                statements
            }
            // The default `Ddl::change` cannot be reached from here — the blanket call
            // would resolve back to this implementation and recurse — so the remaining
            // variants are dispatched by hand.
            Change::CreateTable(table) => vec![self.create_table(table)],
            Change::DropTable { name } => vec![self.drop_table(name)],
            Change::DropColumn { table, column } => vec![self.drop_column(table, column)],
            Change::AlterColumn { table, from, to } => self.alter_column(table, from, to),
            Change::AddIndex { table, index } => vec![self.create_index(table, index)],
            Change::DropIndex { table, index } => vec![self.drop_index(table, index)],
            Change::AddUnique { table, unique } => vec![self.add_unique(table, unique)],
            Change::DropUnique { table, unique } => vec![self.drop_unique(table, unique)],
            Change::AddForeignKey { table, foreign_key } => {
                vec![self.add_foreign_key(table, foreign_key)]
            }
            Change::DropForeignKey { table, foreign_key } => {
                vec![self.drop_foreign_key(table, foreign_key)]
            }
            Change::AddCheck { table, check } => vec![self.add_check(table, check)],
            Change::DropCheck { table, check } => vec![self.drop_check(table, check)],
            Change::CreateView(view) => self.create_view(view),
            Change::ReplaceView(view) => self.replace_view(view),
            Change::DropView { name, materialised } => self.drop_view(name, *materialised),
            // MySQL has no named enum types (an enum is spelled inline in the column) and no
            // sequences, so those render as nothing.
            Change::CreateEnum(_)
            | Change::DropEnum { .. }
            | Change::AddEnumValue { .. }
            | Change::RemoveEnumValue { .. }
            | Change::CreateSequence(_)
            | Change::AlterSequence { .. }
            | Change::DropSequence { .. } => Vec::new(),
            // `Change` is non-exhaustive: a change added later renders as nothing rather
            // than failing to compile, which keeps a schema shared across dialects usable.
            _ => Vec::new(),
        }
    }

    /// Render a diff as statements, with the schemas it was computed from in reach.
    ///
    /// MySQL needs the table definition for an added index, because a `TEXT` or an oversized
    /// `VARCHAR` column can only be keyed with a prefix length, and the index alone does not
    /// carry the column types.
    fn apply_with(&self, changes: &[Change], current: &Schema, desired: &Schema) -> Vec<String> {
        let mut statements = Vec::new();
        for change in changes {
            match change {
                Change::AddIndex { table, index } => {
                    let definition = desired.get(table).or_else(|| current.get(table));
                    statements.extend(self.add_index(table, index, definition));
                }
                other => statements.extend(self.change(other)),
            }
        }
        statements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use besu_schema::Unique;

    #[test]
    fn a_timestamp_with_a_zone_converges() {
        // `DateTime<Utc>` maps to `TimestampTz`, which MySQL stores as `DATETIME(6)` and
        // reports back as `Timestamp`. Folding the two is what keeps `push` a no-op.
        let ddl = MySqlDdl;
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::TimestampTz),
            TypeDef::Timestamp
        );
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Timestamp),
            TypeDef::Timestamp
        );
    }

    #[test]
    fn an_inline_enum_keeps_its_labels_and_forgets_its_name() {
        // MySQL has no named enum, so the name describes nothing and introspection reports
        // none. Dropping it is what makes a declared enum converge; the labels stay, so a
        // changed label set is still a change.
        let declared = TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy", "sad"]));
        let introspected = TypeDef::Enum(besu_core::EnumDef::owned("", ["happy", "sad"]));
        assert_eq!(Ddl::normalise_type(&MySqlDdl, &declared), introspected);
        assert_eq!(Ddl::type_name(&MySqlDdl, &declared), "ENUM('happy','sad')");

        let changed = TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy", "meh", "sad"]));
        assert_ne!(
            Ddl::normalise_type(&MySqlDdl, &declared),
            Ddl::normalise_type(&MySqlDdl, &changed)
        );
    }

    #[test]
    fn bit_and_textual_types_fold_onto_their_storage() {
        let ddl = MySqlDdl;
        // MySQL has one bit type; `VARBIT(n)` is `BIT(n)` there.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::VarBit(6)),
            TypeDef::Bit(6)
        );
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::VarBit(0)),
            TypeDef::Bit(64)
        );
        // An interval and an address have no MySQL type and read back as text.
        for ty in [TypeDef::Interval, TypeDef::Inet, TypeDef::Cidr] {
            assert_eq!(Ddl::normalise_type(&ddl, &ty), TypeDef::Text, "{ty:?}");
        }
        // A `SET` is inline like an enum: the labels are the type, so the fold is identity.
        let labels = besu_core::EnumLabels::from_static(&["a", "b"]);
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Set(labels.clone())),
            TypeDef::Set(labels)
        );
    }

    #[test]
    fn a_domain_folds_onto_its_base_type() {
        let ddl = MySqlDdl;
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Domain(besu_core::DomainDef::new("positive", &TypeDef::Int4))
            ),
            TypeDef::Int4
        );
    }

    fn table() -> Table {
        Table {
            name: "memberships".into(),
            schema: None,
            columns: vec![Column {
                name: "user_id".into(),
                ty: TypeDef::Int8,
                nullable: false,
                auto_increment: false,
                primary_key: false,
                unique: false,
                default: None,
                generated: None,
                comment: None,
                renamed_from: None,
            }],
            indexes: Vec::new(),
            uniques: vec![Unique {
                name: "memberships_user_key".into(),
                columns: vec!["user_id".into()],
            }],
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            comment: None,
        }
    }

    #[test]
    fn a_unique_is_a_named_key_in_create_table() {
        assert_eq!(
            MySqlDdl.create_table(&table()),
            "CREATE TABLE `memberships` (`user_id` BIGINT NOT NULL, \
             CONSTRAINT `memberships_user_key` UNIQUE (`user_id`)) \
             ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
        );
    }

    #[test]
    fn a_unique_is_added_and_dropped_with_alter_table() {
        let unique = &table().uniques[0];
        assert_eq!(
            MySqlDdl.add_unique("memberships", unique),
            "ALTER TABLE `memberships` ADD CONSTRAINT `memberships_user_key` UNIQUE (`user_id`)"
        );
        assert_eq!(
            MySqlDdl.drop_unique("memberships", unique),
            "ALTER TABLE `memberships` DROP CONSTRAINT `memberships_user_key`"
        );
    }

    fn indexed(name: &str, ty: TypeDef, kind: IndexKind, unique: bool) -> (Table, Index) {
        let mut table = Table {
            name: "events".into(),
            schema: None,
            columns: vec![
                Column {
                    name: "id".into(),
                    ty: TypeDef::Int8,
                    nullable: false,
                    auto_increment: false,
                    primary_key: true,
                    unique: false,
                    default: None,
                    generated: None,
                    comment: None,
                    renamed_from: None,
                },
                Column {
                    name: name.into(),
                    ty,
                    nullable: false,
                    auto_increment: false,
                    primary_key: false,
                    unique: false,
                    default: None,
                    generated: None,
                    comment: None,
                    renamed_from: None,
                },
            ],
            indexes: Vec::new(),
            uniques: Vec::new(),
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            comment: None,
        };
        let index = Index {
            name: "events_kind_idx".into(),
            columns: vec![name.into()],
            unique,
            kind,
            predicate: None,
        };
        table.indexes.push(index.clone());
        (table, index)
    }

    #[test]
    fn a_fulltext_key_takes_no_prefix_and_no_unique() {
        // MySQL indexes the whole value for `FULLTEXT`, and has no unique full-text index.
        let (table, _) = indexed("title", TypeDef::Text, IndexKind::FullText, true);
        let sql = MySqlDdl.create_table(&table);
        assert!(
            sql.contains("FULLTEXT KEY `events_kind_idx` (`title`)"),
            "{sql}"
        );
        assert!(!sql.contains("UNIQUE KEY `events_kind_idx`"), "{sql}");
        assert!(!sql.contains("`title`(255)"), "{sql}");
    }

    #[test]
    fn a_spatial_key_is_its_own_spelling() {
        let (table, _) = indexed(
            "at",
            TypeDef::Custom("point".into()),
            IndexKind::Spatial,
            false,
        );
        let sql = MySqlDdl.create_table(&table);
        assert!(
            sql.contains("SPATIAL KEY `events_kind_idx` (`at`)"),
            "{sql}"
        );
    }

    #[test]
    fn a_wide_varchar_unique_is_prefixed() {
        // `VARCHAR(1000)` in `utf8mb4` is 4000 bytes at worst, over InnoDB's 3072-byte key
        // limit. 768 characters is the most that fits.
        let (table, _) = indexed("email", TypeDef::VarChar(1000), IndexKind::Btree, true);
        let sql = MySqlDdl.create_table(&table);
        assert!(sql.contains("`email` VARCHAR(1000) NOT NULL"), "{sql}");
        assert!(
            sql.contains("UNIQUE KEY `events_kind_idx` (`email`(768))"),
            "{sql}"
        );
    }

    #[test]
    fn a_functional_key_part_is_parenthesised() {
        let (mut table, _) = indexed("payload", TypeDef::Text, IndexKind::Btree, false);
        table.indexes[0].columns = vec![
            IndexColumn::column("payload"),
            IndexColumn::expression("lower(payload)"),
        ];
        let sql = MySqlDdl.create_table(&table);
        assert!(
            sql.contains("KEY `events_kind_idx` (`payload`(255), (lower(payload)))"),
            "{sql}"
        );
    }

    #[test]
    fn an_added_index_looks_its_columns_up() {
        // `ApplyWith` is what `push` calls; unlike `create_index` it has the table, which is
        // what makes a `TEXT` key legal.
        let (table, index) = indexed("title", TypeDef::Text, IndexKind::Btree, false);
        let desired = Schema {
            tables: vec![table],
            ..Schema::default()
        };
        let changes = [Change::AddIndex {
            table: "events".into(),
            index,
        }];
        assert_eq!(
            MySqlDdl.apply_with(&changes, &Schema::new(), &desired),
            ["CREATE INDEX `events_kind_idx` ON `events` (`title`(255))"]
        );
    }
}
