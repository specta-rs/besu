use besu_core::{TypeDef, Value};

use crate::{
    Column, Enum, ForeignKey, Index, ReferenceAction, Schema, Table, Unique, diff::Change,
};

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
        sql.push_str(&self.quote_table(&table.qualified_name()));
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

        for unique in &table.uniques {
            sql.push_str(", ");
            sql.push_str(&self.unique_clause(unique));
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

    /// `ALTER TABLE ... RENAME COLUMN`.
    fn rename_column(&self, table: &str, from: &str, to: &str) -> String {
        format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            self.quote_table(table),
            self.quote(from),
            self.quote(to)
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
    /// SQLite has no such statement: changing a column means rebuilding the table, which it
    /// does in [`Ddl::apply_with`]. A dialect that cannot do either returns `false`, and
    /// `push` refuses to alter a column and asks for an explicit migration instead of
    /// quietly dropping data.
    fn can_alter_column(&self) -> bool {
        true
    }

    /// Statements that migrate a column's definition.
    ///
    /// The default is portable and deliberately blunt: drop the old column and add the new
    /// one. Dialects that can do better override this. SQLite does not, because it needs
    /// the rest of the table and so rebuilds it in [`Ddl::apply_with`] instead.
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

    /// The `UNIQUE` clause used inside `CREATE TABLE`.
    fn unique_clause(&self, unique: &Unique) -> String {
        let mut sql = String::from("CONSTRAINT ");
        sql.push_str(&self.quote(&unique.name));
        sql.push_str(" UNIQUE (");
        sql.push_str(&join_quoted(&unique.columns, |c| self.quote(c)));
        sql.push(')');
        sql
    }

    /// `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE`.
    ///
    /// SQLite has no such statement, so it renders uniques inside `CREATE TABLE` instead
    /// and returns no statement here — exactly as it does for foreign keys.
    fn add_unique(&self, table: &str, unique: &Unique) -> String {
        format!(
            "ALTER TABLE {} ADD {}",
            self.quote_table(table),
            self.unique_clause(unique)
        )
    }

    /// `ALTER TABLE ... DROP CONSTRAINT`.
    ///
    /// Like [`Ddl::add_unique`], SQLite returns no statement.
    fn drop_unique(&self, table: &str, unique: &Unique) -> String {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            self.quote_table(table),
            self.quote(&unique.name)
        )
    }

    /// Statements that create a named enum type.
    ///
    /// The default returns nothing, because only PostgreSQL has named types that are not
    /// part of a column: MySQL spells an enum inline in the column and SQLite has no enum
    /// at all. A dialect without them ignores [`Change::CreateEnum`] rather than failing,
    /// which keeps a schema shared with PostgreSQL usable everywhere.
    fn create_enum(&self, _e: &Enum) -> Vec<String> {
        Vec::new()
    }

    /// Statements that drop a named enum type.
    ///
    /// The default returns nothing; see [`Ddl::create_enum`].
    fn drop_enum(&self, _name: &str) -> Vec<String> {
        Vec::new()
    }

    /// Statements that append a value to an existing enum type.
    ///
    /// The default returns nothing; see [`Ddl::create_enum`].
    fn add_enum_value(&self, _name: &str, _value: &str) -> Vec<String> {
        Vec::new()
    }

    /// Statements that remove a value from an existing enum type.
    ///
    /// The default returns nothing, and PostgreSQL overrides it with nothing too: there is
    /// no `ALTER TYPE ... DROP VALUE`. It is a [`Safety::Destructive`](crate::Safety)
    /// change, so `push` refuses it and a migration has to recreate the type by hand.
    fn remove_enum_value(&self, _name: &str, _value: &str) -> Vec<String> {
        Vec::new()
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
                        statements.push(self.create_index(&table.qualified_name(), index));
                    }
                }
                statements
            }
            Change::DropTable { name } => vec![self.drop_table(name)],
            Change::CreateEnum(e) => self.create_enum(e),
            Change::DropEnum { name } => self.drop_enum(name),
            Change::AddEnumValue { name, value } => self.add_enum_value(name, value),
            Change::RemoveEnumValue { name, value } => self.remove_enum_value(name, value),
            Change::AddColumn { table, column } => vec![self.add_column(table, column)],
            Change::RenameColumn { table, from, to } => {
                vec![self.rename_column(table, from, to)]
            }
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
        }
    }

    /// Render a whole schema from scratch.
    ///
    /// Enums come first, because a column can name one; then tables in dependency order;
    /// then their indexes, so a foreign key can reference a table that has not been created
    /// yet.
    fn create_schema(&self, schema: &Schema) -> Vec<String> {
        let mut statements = Vec::new();
        for e in &schema.enums {
            statements.extend(self.create_enum(e));
        }
        statements.extend(
            schema
                .topological_order()
                .into_iter()
                .flat_map(|table| self.default_change(&Change::CreateTable(table.clone()))),
        );
        statements
    }

    /// Render a diff as statements, in order.
    fn apply(&self, changes: &[Change]) -> Vec<String> {
        changes
            .iter()
            .flat_map(|change| self.change(change))
            .collect()
    }

    /// Render a diff as statements, with the schemas it was computed from in reach.
    ///
    /// [`Ddl::apply`] sees only the changes, which is enough for a dialect with a real
    /// `ALTER TABLE ... ALTER COLUMN`: the change carries both the old and the new column.
    /// A dialect that has to rebuild the whole table also needs the table's other columns,
    /// and no [`Change`] carries those, so it overrides this instead. The default ignores
    /// the schemas and defers to [`Ddl::apply`].
    fn apply_with(&self, changes: &[Change], _current: &Schema, _desired: &Schema) -> Vec<String> {
        self.apply(changes)
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
