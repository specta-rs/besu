use besu_core::{TypeDef, Value};

use crate::{Column, ForeignKey, Index, ReferenceAction, Schema, Table, diff::Change};

/// Render a schema as DDL for a particular database.
///
/// Every method has a working default built from the same few primitives, so a new dialect
/// only has to override the parts that genuinely differ. The defaults produce portable SQL
/// that SQLite, PostgreSQL and MySQL all accept; the three shipped dialects override
/// column rendering (auto-increment syntax is wildly different) and `ALTER COLUMN`.
pub trait Ddl {
    /// The dialect's name, for diagnostics.
    fn name(&self) -> &'static str;

    /// Quote an identifier.
    fn quote(&self, ident: &str) -> String;

    /// Quote a possibly schema-qualified table name.
    fn quote_table(&self, table: &str) -> String {
        match table.split_once('.') {
            Some((schema, table)) => format!("{}.{}", self.quote(schema), self.quote(table)),
            None => self.quote(table),
        }
    }

    /// The native type name for a canonical type.
    fn type_name(&self, ty: &TypeDef) -> String;

    /// Fold types the dialect cannot tell apart, so diffing converges.
    ///
    /// SQLite stores every integer as `INTEGER`; a schema declaring `i32` and a database
    /// reporting the column back must compare equal. The default is the identity, which is
    /// right for any dialect that keeps types faithfully.
    fn normalise_type(&self, ty: &TypeDef) -> TypeDef {
        ty.clone()
    }

    /// The full definition of a column, as it appears in `CREATE TABLE`.
    ///
    /// `inline_pk` is true when the table has exactly one primary key column, which is when
    /// dialects expect `PRIMARY KEY` on the column rather than in a table constraint.
    fn column_sql(&self, column: &Column, inline_pk: bool) -> String;

    /// Render a literal value for use in DDL, for example a default.
    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(v) => if *v { "TRUE" } else { "FALSE" }.into(),
            Value::Int(v) => v.to_string(),
            Value::UInt(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Text(v) => format!("'{}'", v.replace('\'', "''")),
            other => format!("{other:?}"),
        }
    }

    /// `CREATE TABLE`.
    fn create_table(&self, table: &Table) -> String {
        let mut sql = String::new();
        sql.push_str("CREATE TABLE ");
        sql.push_str(&self.quote_table(&table.name));
        sql.push_str(" (");

        let primary_key: Vec<&Column> = table.primary_key();
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

        for fk in &table.foreign_keys {
            sql.push_str(", ");
            sql.push_str(&self.foreign_key_clause(fk));
        }

        sql.push(')');
        sql
    }

    /// `DROP TABLE`.
    fn drop_table(&self, table: &str) -> String {
        format!("DROP TABLE {}", self.quote_table(table))
    }

    /// `ALTER TABLE ... ADD COLUMN`.
    fn add_column(&self, table: &str, column: &Column) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {}",
            self.quote_table(table),
            self.column_sql(column, false)
        )
    }

    /// `ALTER TABLE ... DROP COLUMN`.
    fn drop_column(&self, table: &str, column: &str) -> String {
        format!(
            "ALTER TABLE {} DROP COLUMN {}",
            self.quote_table(table),
            self.quote(column)
        )
    }

    /// Whether [`Ddl::create_table`] already includes the table's secondary indexes.
    ///
    /// MySQL needs this because its index syntax differs per column type (a `TEXT` index
    /// needs a prefix length), so it renders them inside `CREATE TABLE`. Everything else
    /// creates them afterwards.
    const INLINE_INDEXES: bool = false;

    /// Whether `ALTER TABLE ... ALTER COLUMN` is available at all.
    ///
    /// SQLite has no such statement: changing a column means rebuilding the table. When
    /// this is `false`, `push` refuses to alter a column and asks for an explicit
    /// migration instead of quietly dropping data.
    fn can_alter_column(&self) -> bool {
        true
    }

    /// Statements that migrate a column's definition.
    ///
    /// The default is portable and deliberately blunt: drop the old column and add the new
    /// one. Dialects that can do better override this.
    fn alter_column(&self, table: &str, from: &Column, to: &Column) -> Vec<String> {
        vec![
            self.drop_column(table, &from.name),
            self.add_column(table, to),
        ]
    }

    /// `CREATE INDEX`.
    fn create_index(&self, table: &str, index: &Index) -> String {
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

    /// `DROP INDEX`.
    fn drop_index(&self, _table: &str, index: &Index) -> String {
        format!("DROP INDEX {}", self.quote(&index.name))
    }

    /// The `FOREIGN KEY` clause used inside `CREATE TABLE`.
    fn foreign_key_clause(&self, fk: &ForeignKey) -> String {
        let mut sql = String::from("CONSTRAINT ");
        sql.push_str(&self.quote(&fk.name));
        sql.push_str(" FOREIGN KEY (");
        sql.push_str(&join_quoted(&fk.columns, |c| self.quote(c)));
        sql.push_str(") REFERENCES ");
        sql.push_str(&self.quote_table(&fk.references_table));
        sql.push_str(" (");
        sql.push_str(&join_quoted(&fk.references_columns, |c| self.quote(c)));
        sql.push(')');
        if let Some(action) = fk.on_delete.as_sql() {
            sql.push_str(" ON DELETE ");
            sql.push_str(action);
        }
        if let Some(action) = fk.on_update.as_sql() {
            sql.push_str(" ON UPDATE ");
            sql.push_str(action);
        }
        sql
    }

    /// `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY`.
    fn add_foreign_key(&self, table: &str, fk: &ForeignKey) -> String {
        format!(
            "ALTER TABLE {} ADD {}",
            self.quote_table(table),
            self.foreign_key_clause(fk)
        )
    }

    /// `ALTER TABLE ... DROP CONSTRAINT`.
    fn drop_foreign_key(&self, table: &str, fk: &ForeignKey) -> String {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            self.quote_table(table),
            self.quote(&fk.name)
        )
    }

    /// Render one change as one or more statements.
    ///
    /// Override this only when a dialect needs different statements for a *subset* of
    /// changes; call [`Ddl::default_change`] for everything else. Overriding it and calling
    /// `self.change(..)` from inside recurses forever.
    fn change(&self, change: &Change) -> Vec<String> {
        self.default_change(change)
    }

    /// The portable implementation of [`Ddl::change`].
    fn default_change(&self, change: &Change) -> Vec<String> {
        match change {
            Change::CreateTable(table) => {
                let mut statements = vec![self.create_table(table)];
                if !Self::INLINE_INDEXES {
                    for index in &table.indexes {
                        statements.push(self.create_index(&table.name, index));
                    }
                }
                statements
            }
            Change::DropTable { name } => vec![self.drop_table(name)],
            Change::AddColumn { table, column } => vec![self.add_column(table, column)],
            Change::DropColumn { table, column } => vec![self.drop_column(table, column)],
            Change::AlterColumn { table, from, to } => self.alter_column(table, from, to),
            Change::AddIndex { table, index } => vec![self.create_index(table, index)],
            Change::DropIndex { table, index } => vec![self.drop_index(table, index)],
            Change::AddForeignKey { table, foreign_key } => {
                vec![self.add_foreign_key(table, foreign_key)]
            }
            Change::DropForeignKey { table, foreign_key } => {
                vec![self.drop_foreign_key(table, foreign_key)]
            }
        }
    }

    /// Render a whole schema from scratch.
    ///
    /// Tables come first, in dependency order, and then their indexes, so a foreign key can
    /// reference a table that has not been created yet.
    fn create_schema(&self, schema: &Schema) -> Vec<String> {
        schema
            .topological_order()
            .into_iter()
            .flat_map(|table| self.default_change(&Change::CreateTable(table.clone())))
            .collect()
    }

    /// Render a diff as statements, in order.
    fn apply(&self, changes: &[Change]) -> Vec<String> {
        changes
            .iter()
            .flat_map(|change| self.change(change))
            .collect()
    }
}

fn join_quoted(parts: &[String], quote: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i != 0 {
            out.push_str(", ");
        }
        out.push_str(&quote(part));
    }
    out
}

/// Render a reference action the way most dialects spell it.
pub fn reference_action_sql(action: ReferenceAction) -> Option<&'static str> {
    action.as_sql()
}
