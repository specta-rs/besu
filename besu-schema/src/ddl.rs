use besu_core::{IndexKind, TypeDef, Value};

use crate::{
    Change, Check, Column, Enum, ForeignKey, Index, IndexColumn, IndexTarget, ReferenceAction,
    Schema, Sequence, Table, Unique, View, view_body,
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
            Value::UInt128(v) => v.to_string(),
            Value::Int128(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Text(v) => format!("'{}'", v.replace('\'', "''")),
            Value::Decimal(v) => v.clone(),
            Value::Array { values, .. } => {
                let mut out = String::from("ARRAY[");
                for (i, value) in values.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&self.literal(value));
                }
                out.push(']');
                out
            }
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

        for check in &table.checks {
            sql.push_str(", ");
            sql.push_str(&self.check_clause(check));
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
    ///
    /// The access method is rendered as `USING <method>` for the PostgreSQL ones. MySQL has
    /// its own syntax and overrides this; the two MySQL-only kinds fall back to GIN and
    /// GiST here, which is the closest thing PostgreSQL has to a full-text and a spatial
    /// index.
    fn create_index(&self, table: &str, index: &Index) -> String {
        let mut sql = String::from("CREATE ");
        if index.unique && !matches!(index.kind, IndexKind::FullText) {
            sql.push_str("UNIQUE ");
        }
        sql.push_str("INDEX ");
        sql.push_str(&self.quote(&index.name));
        sql.push_str(" ON ");
        sql.push_str(&self.quote_table(table));

        let method = match index.kind {
            IndexKind::Btree => None,
            IndexKind::FullText => Some("gin"),
            IndexKind::Gin => Some("gin"),
            IndexKind::Spatial => Some("gist"),
            IndexKind::Gist => Some("gist"),
            IndexKind::Brin => Some("brin"),
            // `IndexKind` is non-exhaustive: a kind added later falls back to the default
            // access method rather than failing to render.
            _ => None,
        };
        if let Some(method) = method {
            sql.push_str(" USING ");
            sql.push_str(method);
        }

        sql.push_str(" (");
        for (i, entry) in index.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            sql.push_str(&self.index_column_sql(entry));
        }
        sql.push(')');
        if let Some(predicate) = &index.predicate {
            sql.push_str(" WHERE ");
            sql.push_str(predicate);
        }
        sql
    }

    /// One key part of an index, as a PostgreSQL-style default renders it.
    ///
    /// A column is quoted; an expression is raw SQL, written verbatim, because only the
    /// caller knows what the expression means. An operator class follows it, which is where
    /// PostgreSQL expects it. Dialects without operator classes override this to ignore
    /// them, rather than render a statement they cannot parse.
    fn index_column_sql(&self, entry: &IndexColumn) -> String {
        let mut sql = match &entry.target {
            IndexTarget::Column(name) => self.quote(name),
            IndexTarget::Expression(expression) => self.index_expression_sql(expression),
        };
        if let Some(opclass) = &entry.opclass {
            sql.push(' ');
            sql.push_str(opclass);
        }
        sql
    }

    /// Render the expression of an index key.
    ///
    /// The default writes it verbatim, which is what SQLite and MySQL's functional-key
    /// spelling accept. A dialect whose grammar needs extra parentheses overrides this.
    fn index_expression_sql(&self, expression: &str) -> String {
        expression.to_owned()
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

    /// The `CHECK` clause used inside `CREATE TABLE`.
    fn check_clause(&self, check: &Check) -> String {
        format!(
            "CONSTRAINT {} CHECK ({})",
            self.quote(&check.name),
            check.expression
        )
    }

    /// `ALTER TABLE ... ADD CONSTRAINT ... CHECK`.
    ///
    /// SQLite has no such statement, so it renders checks inside `CREATE TABLE` only and
    /// returns no statement here, exactly as it does for uniques and foreign keys.
    fn add_check(&self, table: &str, check: &Check) -> String {
        format!(
            "ALTER TABLE {} ADD {}",
            self.quote_table(table),
            self.check_clause(check)
        )
    }

    /// `ALTER TABLE ... DROP CONSTRAINT`.
    ///
    /// Like [`Ddl::add_check`], SQLite returns no statement.
    fn drop_check(&self, table: &str, check: &Check) -> String {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            self.quote_table(table),
            self.quote(&check.name)
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

    /// Statements that create a view.
    ///
    /// A view carries its definition as SQL, so the statement is the header plus that body.
    /// MySQL and SQLite have no materialised views and override this to ignore the flag
    /// rather than emit a spelling their server rejects.
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

    /// Statements that replace an existing view.
    ///
    /// The default uses `CREATE OR REPLACE VIEW`, which PostgreSQL and MySQL understand.
    /// SQLite has no `OR REPLACE` and overrides this with a drop and a create.
    fn replace_view(&self, view: &View) -> Vec<String> {
        let materialised = if view.materialised {
            "MATERIALIZED "
        } else {
            ""
        };
        vec![format!(
            "CREATE OR REPLACE {materialised}VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// Statements that drop a view.
    ///
    /// A materialised view has its own `DROP` spelling in every dialect that has one, so
    /// the flag travels with the change.
    fn drop_view(&self, name: &str, materialised: bool) -> Vec<String> {
        let materialised = if materialised { "MATERIALIZED " } else { "" };
        vec![format!(
            "DROP {materialised}VIEW {}",
            self.quote_table(name)
        )]
    }

    /// Statements that rebuild a materialised view's contents.
    ///
    /// The default returns nothing, because only PostgreSQL has materialised views. It is
    /// emitted as part of replacing one: `CREATE MATERIALIZED VIEW ... AS` already populates
    /// the result, but an explicit refresh makes the cost of the change visible in the
    /// generated migration.
    fn refresh_view(&self, _view: &View) -> Vec<String> {
        Vec::new()
    }

    /// Statements that create a sequence.
    ///
    /// The default returns nothing: SQLite and MySQL have no standalone sequence, so a
    /// snapshot shared with PostgreSQL still pushes to them. PostgreSQL implements it.
    fn create_sequence(&self, _sequence: &Sequence) -> Vec<String> {
        Vec::new()
    }

    /// Statements that change a sequence's options.
    ///
    /// The default returns nothing; see [`Ddl::create_sequence`].
    fn alter_sequence(&self, _from: &Sequence, _to: &Sequence) -> Vec<String> {
        Vec::new()
    }

    /// Statements that drop a sequence.
    ///
    /// The default returns nothing; see [`Ddl::create_sequence`].
    fn drop_sequence(&self, _name: &str) -> Vec<String> {
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
            Change::AddCheck { table, check } => vec![self.add_check(table, check)],
            Change::DropCheck { table, check } => vec![self.drop_check(table, check)],
            Change::CreateView(view) => self.create_view(view),
            Change::ReplaceView(view) => self.replace_view(view),
            Change::DropView { name, materialised } => self.drop_view(name, *materialised),
            Change::CreateSequence(sequence) => self.create_sequence(sequence),
            Change::AlterSequence { from, to, .. } => self.alter_sequence(from, to),
            Change::DropSequence { name } => self.drop_sequence(name),
        }
    }

    /// Render a whole schema from scratch.
    ///
    /// Sequences and enums come first, because a column can name or default to one; then
    /// tables in dependency order; then their indexes, so a foreign key can reference a
    /// table that has not been created yet; and finally views, in dependency order, so a
    /// view over a table created here works.
    fn create_schema(&self, schema: &Schema) -> Vec<String> {
        let mut statements = Vec::new();
        for sequence in &schema.sequences {
            statements.extend(self.create_sequence(sequence));
        }
        for e in &schema.enums {
            statements.extend(self.create_enum(e));
        }
        statements.extend(
            schema
                .topological_order()
                .into_iter()
                .flat_map(|table| self.default_change(&Change::CreateTable(table.clone()))),
        );
        statements.extend(
            schema
                .ordered_views()
                .into_iter()
                .flat_map(|view| self.create_view(view)),
        );
        statements
    }

    /// Render a diff as statements, in order.
    ///
    /// Prefer [`apply_with`](Ddl::apply_with) when you have the schemas: a dialect may need a
    /// table's column types to render a change. MySQL is the case in point — an index over a
    /// `TEXT` column needs a prefix length, which the index alone does not carry, so this
    /// method (which sees only the changes) cannot produce the right statement for it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IndexKind;

    /// The portable default, which is what a PostgreSQL-shaped dialect renders.
    struct TestDdl;

    impl Ddl for TestDdl {
        fn name(&self) -> &'static str {
            "test"
        }

        fn quote(&self, ident: &str) -> String {
            format!("\"{ident}\"")
        }

        fn type_name(&self, _ty: &TypeDef) -> String {
            "TEXT".into()
        }

        fn column_sql(&self, column: &Column, _inline_pk: bool) -> String {
            self.quote(&column.name)
        }
    }

    #[test]
    fn an_index_renders_its_entries_in_order() {
        let index = Index {
            name: "users_search_idx".into(),
            columns: vec![
                IndexColumn::column("email").with_opclass("text_pattern_ops"),
                IndexColumn::expression("lower(name)"),
            ],
            unique: true,
            kind: IndexKind::Gin,
            predicate: Some("active".into()),
        };

        assert_eq!(
            TestDdl.create_index("users", &index),
            r#"CREATE UNIQUE INDEX "users_search_idx" ON "users" USING gin ("email" text_pattern_ops, lower(name)) WHERE active"#
        );
    }

    #[test]
    fn a_plain_column_has_no_operator_class_and_no_expression_parentheses() {
        let index = Index {
            name: "users_email_idx".into(),
            columns: vec![IndexColumn::column("email")],
            unique: false,
            kind: IndexKind::Btree,
            predicate: None,
        };

        assert_eq!(
            TestDdl.create_index("users", &index),
            r#"CREATE INDEX "users_email_idx" ON "users" ("email")"#
        );
    }

    fn accounts(checks: Vec<Check>) -> Table {
        Table {
            name: "accounts".into(),
            schema: None,
            columns: vec![Column {
                name: "balance".into(),
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
            uniques: Vec::new(),
            foreign_keys: Vec::new(),
            checks,
            comment: None,
        }
    }

    #[test]
    fn a_check_is_written_inside_create_table_and_alters_by_name() {
        let check = Check {
            name: "accounts_balance_check".into(),
            expression: "balance > 0".into(),
        };
        let table = accounts(vec![check.clone()]);

        assert_eq!(
            TestDdl.create_table(&table),
            r#"CREATE TABLE "accounts" ("balance", CONSTRAINT "accounts_balance_check" CHECK (balance > 0))"#
        );
        assert_eq!(
            TestDdl.add_check("accounts", &check),
            r#"ALTER TABLE "accounts" ADD CONSTRAINT "accounts_balance_check" CHECK (balance > 0)"#
        );
        assert_eq!(
            TestDdl.drop_check("accounts", &check),
            r#"ALTER TABLE "accounts" DROP CONSTRAINT "accounts_balance_check""#
        );
    }
}
