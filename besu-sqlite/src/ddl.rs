use besu_core::{TypeDef, Value};
use besu_schema::{Change, Column, Ddl, Schema, Table};

use crate::SQLite;

/// DDL generation for SQLite.
#[derive(Debug, Clone, Copy, Default)]
pub struct SQLiteDdl;

impl SQLiteDdl {
    /// `CREATE TABLE` for `table`, written under `name` instead of the table's own name.
    ///
    /// The table rebuild is the only caller: it creates the desired table under a temporary
    /// name, then renames it once the old table is gone.
    fn create_table_as(&self, table: &Table, name: &str) -> String {
        let mut temporary = table.clone();
        temporary.name = name.to_owned();
        self.create_table(&temporary)
    }

    /// The standard SQLite table rebuild: create the desired table under a temporary name,
    /// copy the rows across, drop the old table, rename the new one into place, then
    /// recreate the indexes.
    fn rebuild_table(&self, current: &Table, desired: &Table) -> Vec<String> {
        let temporary = format!("{}__besu_tmp", desired.name);

        let mut statements = vec![
            // `DROP TABLE` would fail while another table's foreign key points at this
            // one, and with an `ON DELETE CASCADE` it would delete the child rows instead.
            // Enforcement is therefore off for the whole rebuild. SQLite ignores this
            // pragma inside a transaction, and `Database::push` executes statements one at
            // a time without opening one, so the setting takes effect.
            "PRAGMA foreign_keys = OFF".to_owned(),
            self.create_table_as(desired, &temporary),
        ];

        // Only the columns present in both definitions are copied. A dropped column is
        // therefore not selected, and a column added by the same push keeps the default
        // that `CREATE TABLE` gave it. A rename is not an `AlterColumn` in this codebase
        // (issue #54), so there is nothing to guess; a renamed column is simply a dropped
        // one plus an added one.
        let columns: Vec<String> = desired
            .columns
            .iter()
            .filter(|column| current.column(&column.name).is_some())
            .map(|column| self.quote(&column.name))
            .collect();

        // An `INSERT` with an empty column list is not valid SQL. It cannot happen for a
        // diff: an `AlterColumn` means the column exists on both sides, so at least one
        // name is always shared.
        if !columns.is_empty() {
            let columns = columns.join(", ");
            statements.push(format!(
                "INSERT INTO {} ({columns}) SELECT {columns} FROM {}",
                self.quote_table(&temporary),
                self.quote_table(&desired.name),
            ));
        }

        statements.push(self.drop_table(&desired.name));
        // `PRAGMA legacy_alter_table` is not needed: the old table is gone before this
        // rename, so there are no references to the temporary name to rewrite.
        statements.push(format!(
            "ALTER TABLE {} RENAME TO {}",
            self.quote_table(&temporary),
            self.quote(&desired.name),
        ));

        // `DROP TABLE` took the old indexes with it. Unique column constraints are inline
        // in `CREATE TABLE`; the secondary ones have to be recreated here.
        for index in &desired.indexes {
            statements.push(self.create_index(&desired.name, index));
        }

        statements.push("PRAGMA foreign_keys = ON".to_owned());
        statements
    }
}

impl Ddl for SQLiteDdl {
    fn name(&self) -> &'static str {
        "sqlite"
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
        use besu_core::Dialect;
        SQLite::type_name(ty.clone()).into_owned()
    }

    fn normalise_type(&self, ty: &TypeDef) -> TypeDef {
        // SQLite has one integer type, one text type and one blob type, so everything else
        // is a matter of intent rather than storage. Folding here is what lets a schema that
        // declares `i32`, `VARCHAR(20)` or `TIMESTAMPTZ` converge against the database.
        match ty {
            TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 => TypeDef::Int8,
            TypeDef::UInt(_) => TypeDef::Int8,
            TypeDef::Char(_) | TypeDef::VarChar(_) | TypeDef::Decimal(_, _) => TypeDef::Text,
            TypeDef::Jsonb => TypeDef::Json,
            TypeDef::TimestampTz => TypeDef::Timestamp,
            other => other.clone(),
        }
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!(
            "{} {}",
            self.quote(&column.name),
            self.type_name(&column.ty)
        );

        // SQLite only accepts `AUTOINCREMENT` on `INTEGER PRIMARY KEY`, and it must be
        // written inline.
        if column.auto_increment && column.primary_key {
            sql.push_str(" PRIMARY KEY AUTOINCREMENT");
            return sql;
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        if !column.nullable && !column.primary_key {
            sql.push_str(" NOT NULL");
        }

        if column.unique {
            sql.push_str(" UNIQUE");
        }

        if let Some(default) = &column.default {
            sql.push_str(" DEFAULT ");
            sql.push_str(default);
        }

        sql
    }

    fn can_alter_column(&self) -> bool {
        // Not `ALTER TABLE ... ALTER COLUMN` — SQLite has no such statement — but
        // `apply_with` can rebuild the table losslessly, which is what this flag promises.
        true
    }

    fn apply_with(&self, changes: &[Change], current: &Schema, desired: &Schema) -> Vec<String> {
        // A table that has an altered column is rebuilt once, from its full desired
        // definition, so every other change for that table is skipped: the rebuild already
        // creates the desired columns, inline constraints, foreign keys and indexes, and a
        // later `ADD COLUMN`, `ADD INDEX` or `DROP INDEX` would fail or apply twice.
        let altered: Vec<&str> = changes
            .iter()
            .filter_map(|change| match change {
                Change::AlterColumn { table, .. } => Some(table.as_str()),
                _ => None,
            })
            .collect();

        let mut rebuilt: Vec<&str> = Vec::new();
        let mut statements = Vec::new();

        for change in changes {
            let table = change.table();
            if altered.contains(&table) {
                if rebuilt.contains(&table) {
                    continue;
                }
                // A diff only emits `AlterColumn` for a table that exists on both sides.
                if let (Some(from), Some(to)) = (current.table(table), desired.table(table)) {
                    rebuilt.push(table);
                    statements.extend(self.rebuild_table(from, to));
                }
                continue;
            }
            statements.extend(self.change(change));
        }

        statements
    }

    fn drop_index(&self, _table: &str, index: &besu_schema::Index) -> String {
        format!("DROP INDEX {}", self.quote(&index.name))
    }

    fn add_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`.
        String::new()
    }

    fn drop_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        String::new()
    }

    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(v) => if *v { "1" } else { "0" }.into(),
            other => Ddl::literal(self, other),
        }
    }
}
