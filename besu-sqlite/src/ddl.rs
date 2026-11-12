use besu_core::{TypeDef, Value};
use besu_schema::{Change, Check, Column, Ddl, Schema, Table};

use crate::SQLite;

/// DDL generation for SQLite.
#[derive(Debug, Clone, Copy, Default)]
pub struct SQLiteDdl;

impl SQLiteDdl {
    /// The standard SQLite table rebuild: create the desired table under a temporary name,
    /// copy the rows across, drop the old table, rename the new one into place, then
    /// recreate the indexes.
    fn rebuild_table(&self, current: &Table, desired: &Table) -> Vec<String> {
        // The temporary table keeps the schema, so a table in an attached database is
        // rebuilt there rather than in the main one.
        let mut temporary = desired.clone();
        temporary.name = format!("{}__besu_tmp", desired.name);

        let mut statements = vec![
            // `DROP TABLE` would fail while another table's foreign key points at this
            // one, and with an `ON DELETE CASCADE` it would delete the child rows instead.
            // Enforcement is therefore off for the whole rebuild. SQLite ignores this
            // pragma inside a transaction, and `Database::push` executes statements one at
            // a time without opening one, so the setting takes effect.
            "PRAGMA foreign_keys = OFF".to_owned(),
            self.create_table(&temporary),
        ];

        // Only the columns present in both definitions are copied. A dropped column is
        // therefore not selected, and a column added by the same push keeps the default
        // that `CREATE TABLE` gave it. A generated column is never written: the database
        // computes it, and SQLite rejects an `INSERT` into one.
        let columns: Vec<String> = desired
            .columns
            .iter()
            .filter(|column| column.generated.is_none())
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
                self.quote_table(&temporary.qualified_name()),
                self.quote_table(&desired.qualified_name()),
            ));
        }

        statements.push(self.drop_table(&desired.qualified_name()));
        // `PRAGMA legacy_alter_table` is not needed: the old table is gone before this
        // rename, so there are no references to the temporary name to rewrite. The new name
        // cannot be schema-qualified; the table stays where it already is.
        statements.push(format!(
            "ALTER TABLE {} RENAME TO {}",
            self.quote_table(&temporary.qualified_name()),
            self.quote(&desired.name),
        ));

        // `DROP TABLE` took the old indexes with it. Unique column constraints, multi-column
        // uniques, checks and foreign keys are inline in `CREATE TABLE`; the secondary
        // indexes have to be recreated here.
        for index in &desired.indexes {
            statements.push(self.create_index(&desired.qualified_name(), index));
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
            // A 128-bit value is stored as decimal text, so it converges against `TEXT`.
            TypeDef::UInt(128) => TypeDef::Text,
            TypeDef::UInt(_) => TypeDef::Int8,
            TypeDef::Char(_) | TypeDef::VarChar(_) | TypeDef::Decimal(_, _) => TypeDef::Text,
            TypeDef::Jsonb => TypeDef::Json,
            TypeDef::TimestampTz => TypeDef::Timestamp,
            // SQLite keeps no precision in the declared type, so a snapshot that carries one
            // has to compare equal to the plain type the database reports back.
            TypeDef::TimeP(_) | TypeDef::TimeTz | TypeDef::TimeTzP(_) => TypeDef::Time,
            TypeDef::TimestampP(_) | TypeDef::TimestampTzP(_) => TypeDef::Timestamp,
            // A bit string is stored as an integer and an array as JSON text.
            TypeDef::Bit(_) | TypeDef::VarBit(_) => TypeDef::Int8,
            TypeDef::Array(_) => TypeDef::Json,
            TypeDef::Collated(inner, collation) => {
                TypeDef::Collated(Box::new(self.normalise_type(inner)), collation.clone())
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

        // SQLite only accepts `AUTOINCREMENT` on `INTEGER PRIMARY KEY`, and it must be
        // written inline.
        if column.auto_increment && column.primary_key {
            sql.push_str(" PRIMARY KEY AUTOINCREMENT");
            return sql;
        }

        if let Some(generated) = &column.generated {
            // SQLite implements both spellings; only `STORED` occupies space and may be
            // indexed.
            sql.push_str(" GENERATED ALWAYS AS (");
            sql.push_str(&generated.expression);
            sql.push_str(if generated.stored {
                ") STORED"
            } else {
                ") VIRTUAL"
            });
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        if !column.nullable && !column.primary_key {
            sql.push_str(" NOT NULL");
        }

        if column.unique && column.generated.is_none() {
            sql.push_str(" UNIQUE");
        }

        // A generated column is computed from its expression and SQLite rejects a default
        // beside one.
        if column.generated.is_none()
            && let Some(default) = &column.default
        {
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
        let altered: Vec<String> = changes
            .iter()
            .filter_map(|change| match change {
                Change::AlterColumn { table, .. } => Some(table.clone()),
                // `ALTER TABLE ... ADD COLUMN ... STORED` is rejected by SQLite once the
                // table has rows ("cannot add a STORED column"), and STORED is the default.
                // A virtual one is legal, so only the stored case needs the rebuild.
                Change::AddColumn { table, column }
                    if column.generated.as_ref().is_some_and(|g| g.stored) =>
                {
                    Some(table.clone())
                }
                _ => None,
            })
            .collect();

        let mut rebuilt: Vec<String> = Vec::new();
        let mut statements = Vec::new();

        for change in changes {
            // Enum changes carry no table and render as nothing on SQLite.
            let Some(table) = change.table() else {
                statements.extend(self.change(change));
                continue;
            };
            if altered.contains(&table) {
                if rebuilt.contains(&table) {
                    continue;
                }
                // A diff only emits `AlterColumn` for a table that exists on both sides.
                if let (Some(from), Some(to)) = (current.get(&table), desired.get(&table)) {
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

    /// `CREATE INDEX`.
    ///
    /// SQLite has no access methods: every index is a b-tree, whatever kind a shared
    /// snapshot declares. It also has no partial-index predicate in the IR, so a `WHERE`
    /// clause is never rendered on an index SQLite created.
    fn create_index(&self, table: &str, index: &besu_schema::Index) -> String {
        let mut sql = String::from("CREATE ");
        if index.unique {
            sql.push_str("UNIQUE ");
        }
        sql.push_str("INDEX ");
        sql.push_str(&self.quote(&index.name));
        sql.push_str(" ON ");
        sql.push_str(&self.quote_table(table));
        sql.push_str(" (");
        for (i, column) in index.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(&self.quote(column));
        }
        sql.push(')');
        sql
    }

    /// `CREATE VIEW`.
    ///
    /// SQLite has no materialised views, so a snapshot that declares one is created as an
    /// ordinary view. That is documented drift: introspection reports it as plain.
    fn create_view(&self, view: &besu_schema::View) -> Vec<String> {
        vec![format!(
            "CREATE VIEW {} AS {}",
            self.quote_table(&view.name),
            besu_schema::view_body(&view.definition)
        )]
    }

    /// Replace a view by dropping and creating it.
    ///
    /// SQLite has no `CREATE OR REPLACE VIEW`, so a changed definition is two statements.
    /// Dropping the view cannot lose data: the rows are read from the tables it names.
    fn replace_view(&self, view: &besu_schema::View) -> Vec<String> {
        vec![
            format!("DROP VIEW IF EXISTS {}", self.quote_table(&view.name)),
            format!(
                "CREATE VIEW {} AS {}",
                self.quote_table(&view.name),
                besu_schema::view_body(&view.definition)
            ),
        ]
    }

    /// `DROP VIEW`.
    ///
    /// SQLite has no materialised views, so the flag is ignored and the statement SQLite
    /// accepts is always a plain drop.
    fn drop_view(&self, name: &str, _materialised: bool) -> Vec<String> {
        vec![format!("DROP VIEW {}", self.quote_table(name))]
    }

    fn add_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`.
        String::new()
    }

    fn drop_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        String::new()
    }

    fn add_unique(&self, _table: &str, _unique: &besu_schema::Unique) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`; a unique constraint can only be
        // written inside `CREATE TABLE`. Adding one to an existing table therefore means
        // rebuilding it, which `push` does not do for this change.
        String::new()
    }

    fn drop_unique(&self, _table: &str, _unique: &besu_schema::Unique) -> String {
        // See `add_unique`. Dropping the constraint would need the same rebuild.
        String::new()
    }

    fn add_check(&self, _table: &str, _check: &Check) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`; a check can only be written inside
        // `CREATE TABLE`, so adding one to an existing table means rebuilding it. `push` does
        // not rebuild for this change, so the statement is empty.
        String::new()
    }

    fn drop_check(&self, _table: &str, _check: &Check) -> String {
        // See `add_check`: dropping a check would need the same rebuild.
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

#[cfg(test)]
mod tests {
    use super::*;
    use besu_schema::Unique;

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
            rls: false,
        }
    }

    #[test]
    fn a_unique_is_written_inside_create_table() {
        assert_eq!(
            SQLiteDdl.create_table(&table()),
            r#"CREATE TABLE "memberships" ("user_id" INTEGER NOT NULL, CONSTRAINT "memberships_user_key" UNIQUE ("user_id"))"#
        );
    }

    #[test]
    fn a_unique_cannot_be_added_or_dropped_after_the_fact() {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`, so the change renders as nothing
        // and only a table rebuild can apply it.
        let unique = &table().uniques[0];
        assert!(SQLiteDdl.add_unique("memberships", unique).is_empty());
        assert!(SQLiteDdl.drop_unique("memberships", unique).is_empty());
    }
}
