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
    /// The type cannot be indexed at all (`JSON`), so uniqueness cannot be enforced here.
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

    fn quote(&self, ident: &str) -> String {
        MySql::quote(ident)
    }

    fn type_name(&self, ty: &TypeDef) -> String {
        MySql::type_name(ty.clone()).into_owned()
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!("{} {}", self.quote(&column.name), self.type_name(&column.ty));

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

    fn create_table(&self, table: &Table) -> String {
        let mut sql = String::from("CREATE TABLE ");
        sql.push_str(&self.quote_table(&table.name));
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
            other => Ddl::change(self, other),
        }
    }
}
