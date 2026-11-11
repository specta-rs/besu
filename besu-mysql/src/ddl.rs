use besu_core::{Dialect, TypeDef};
use besu_schema::{Change, Column, Ddl, Table};

use crate::MySql;

/// DDL generation for MySQL.
#[derive(Debug, Clone, Copy, Default)]
pub struct MySqlDdl;

/// The prefix length used when `UNIQUE` has to index a `TEXT` or `BLOB` column.
///
/// MySQL cannot index those types without a prefix. 255 characters is under the 3072-byte
/// key limit for `utf8mb4` with InnoDB's default row format, and is what MySQL itself
/// suggests for a unique short-string key.
const INDEX_PREFIX: u32 = 255;

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

fn uniqueness(ty: &TypeDef) -> Uniqueness {
    match ty {
        TypeDef::Text | TypeDef::Bytes => Uniqueness::Prefix(INDEX_PREFIX),
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
            ty,
            TypeDef::Text | TypeDef::Bytes | TypeDef::Json | TypeDef::Jsonb
        )
    }
}

impl Ddl for MySqlDdl {
    fn name(&self) -> &'static str {
        "mysql"
    }

    // MySQL needs a prefix length to index a `TEXT` or `BLOB` column, which `create_index`
    // cannot know without the table, so the indexes are rendered inside `CREATE TABLE`.
    const INLINE_INDEXES: bool = true;

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
            // MySQL has no column type that stores an offset: `DATETIME` is the same column
            // for both, so a declared `TimestampTz` must compare equal to the `Timestamp`
            // introspection reports — otherwise every `push` sees a type change.
            TypeDef::TimestampTz => TypeDef::Timestamp,
            // `u128` is `DECIMAL(39,0)`, which is indistinguishable from a
            // `Decimal(39, 0)`; an `i128` is declared as that already.
            TypeDef::UInt(128) => TypeDef::Decimal(39, 0),
            other => other.clone(),
        }
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!(
            "{} {}",
            self.quote(&column.name),
            self.type_name(&column.ty)
        );

        // MySQL makes primary key columns `NOT NULL` implicitly, but spelling it out keeps
        // the generated `CREATE TABLE` readable and matches `SHOW CREATE TABLE`.
        if !column.nullable && (!column.primary_key || column.auto_increment) {
            sql.push_str(" NOT NULL");
        }

        // `AUTO_INCREMENT` requires the column to be a key, which the inline `PRIMARY KEY`
        // below provides.
        if column.auto_increment {
            sql.push_str(" AUTO_INCREMENT");
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        if column.unique && matches!(uniqueness(&column.ty), Uniqueness::Inline) {
            sql.push_str(" UNIQUE");
        }

        if let Some(default) = &column.default {
            sql.push_str(" DEFAULT ");
            if Self::default_needs_expression(&column.ty) {
                sql.push('(');
                sql.push_str(default);
                sql.push(')');
            } else {
                sql.push_str(default);
            }
        }

        sql
    }

    fn alter_column(&self, table: &str, _from: &Column, to: &Column) -> Vec<String> {
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
            if index.unique {
                sql.push_str("UNIQUE ");
            }
            sql.push_str("KEY ");
            sql.push_str(&self.quote(&index.name));
            sql.push_str(" (");
            for (i, name) in index.columns.iter().enumerate() {
                if i != 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&self.quote(name));
                // The key length of a `TEXT`/`BLOB` column is not known until its type is.
                if let Some(column) = table.column(name) {
                    if let Uniqueness::Prefix(prefix) = uniqueness(&column.ty) {
                        sql.push_str(&format!("({prefix})"));
                    }
                }
            }
            sql.push(')');
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

    fn change(&self, change: &Change) -> Vec<String> {
        match change {
            // `ALTER TABLE ... ADD COLUMN` cannot add an index, so a unique column whose
            // type needs a prefix is followed by a separate `CREATE UNIQUE INDEX`.
            Change::AddColumn { table, column } => {
                let mut statements = vec![self.add_column(table, column)];
                if column.unique {
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
            // MySQL has no named enum types (an enum is spelled inline in the column) and
            // SQLite has none either, so the enum changes render as nothing.
            _ => Vec::new(),
        }
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
}
