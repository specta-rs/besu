use besu_core::{TypeDef, Value};
use besu_schema::{Change, Check, Column, Ddl, Index, Schema, Table};

use crate::{REBUILD_SUFFIX, SQLite};

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
        temporary.name = format!("{}{REBUILD_SUFFIX}", desired.name);

        let mut statements = vec![
            // `DROP TABLE` would fail while another table's foreign key points at this
            // one, and with an `ON DELETE CASCADE` it would delete the child rows instead.
            // Enforcement is therefore off for the whole rebuild. SQLite ignores this
            // pragma inside a transaction, and `Database::push` executes statements one at
            // a time without opening one, so the setting takes effect.
            "PRAGMA foreign_keys = OFF".to_owned(),
            // A rebuild that failed after this point left the temporary table behind, and
            // creating it again would then be the error the next push reports. It is always
            // this rebuild's own scratch table, so whatever is there is a leftover copy.
            format!(
                "DROP TABLE IF EXISTS {}",
                self.quote_table(&temporary.qualified_name())
            ),
            self.create_table(&temporary),
        ];

        // Only the columns present in both definitions are copied. A dropped column is
        // therefore not selected, and a column added by the same push keeps the default
        // that `CREATE TABLE` gave it. A generated column is never written: the database
        // computes it, and SQLite rejects an `INSERT` into one.
        //
        // A column the declaration renamed is copied from its old name: the rebuild is what
        // performs the rename, so selecting the new name from the old table would fail to
        // find it and lose the column's values.
        let columns: Vec<(String, String)> = desired
            .columns
            .iter()
            .filter(|column| column.generated.is_none())
            .filter_map(|column| {
                if current.column(&column.name).is_some() {
                    return Some((self.quote(&column.name), self.quote(&column.name)));
                }
                let previous = column.renamed_from.as_deref()?;
                current
                    .column(previous)
                    .is_some()
                    .then(|| (self.quote(&column.name), self.quote(previous)))
            })
            .collect();

        // An `INSERT` with an empty column list is not valid SQL. It cannot happen for a
        // diff: an `AlterColumn` means the column exists on both sides, so at least one
        // name is always shared.
        if !columns.is_empty() {
            let target = columns
                .iter()
                .map(|(target, _)| target.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let source = columns
                .iter()
                .map(|(_, source)| source.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            statements.push(format!(
                "INSERT INTO {} ({target}) SELECT {source} FROM {}",
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

    /// SQLite has no named enum type: the labels are stored as text.
    fn inline_enums(&self) -> bool {
        true
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
            // A bit string, an interval, an address and a `SET` are all stored as text, and a
            // domain is stored as its base type.
            TypeDef::Bit(_) | TypeDef::VarBit(_) => TypeDef::Text,
            TypeDef::Interval | TypeDef::Inet | TypeDef::Cidr => TypeDef::Text,
            TypeDef::Range(_) => TypeDef::Text,
            // A declared spatial column is stored in a `BLOB`, which is what introspection
            // reports back for it.
            TypeDef::Geometry { .. } | TypeDef::Geography { .. } => TypeDef::Bytes,
            TypeDef::Vector(_) => TypeDef::Text,
            TypeDef::Set(_) => TypeDef::Text,
            TypeDef::Domain(def) => self.normalise_type(def.base.as_type()),
            TypeDef::Array(_) => TypeDef::Json,
            // SQLite has no enum: the label is stored in a `TEXT` column, so a declared enum
            // has to converge against the text type introspection reports.
            TypeDef::Enum(_) => TypeDef::Text,
            TypeDef::Collated(inner, collation) => {
                TypeDef::Collated(Box::new(self.normalise_type(inner)), collation.clone())
            }
            other => other.clone(),
        }
    }

    /// SQLite has only b-trees, no operator classes and no partial indexes in the IR.
    ///
    /// Expression indexes are supported and introspection reads them back, so the entries
    /// themselves are kept; only the parts SQLite cannot express are folded away.
    fn normalise_index(&self, index: &Index) -> Index {
        let mut index = index.clone();
        index.kind = besu_core::IndexKind::Btree;
        index.predicate = None;
        for column in &mut index.columns {
            column.opclass = None;
        }
        index
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
            // An expression is only a valid default inside parentheses: SQLite reads
            // `DEFAULT (1 + 1)` back as `1 + 1`, so the text introspection stores has no
            // brackets, and rendering it verbatim would be a syntax error. A literal must
            // stay bare, because `DEFAULT (0)` reads back as `0` and the bracketed spelling
            // would differ from the declaration on every push.
            if is_simple_default(default) {
                sql.push_str(default);
            } else {
                sql.push('(');
                sql.push_str(default);
                sql.push(')');
            }
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
                // SQLite has no `ALTER TABLE ... ADD/DROP CONSTRAINT`, so a check, a foreign
                // key or a multi-column unique added or removed on an existing table is
                // applied by the same rebuild. `create_table` writes the desired ones, and
                // the copy keeps every row. Without this the change would render as an empty
                // statement: the constraint would silently not exist, and every later push
                // would report it again.
                Change::AddCheck { table, .. }
                | Change::DropCheck { table, .. }
                | Change::AddUnique { table, .. }
                | Change::DropUnique { table, .. }
                | Change::AddForeignKey { table, .. }
                | Change::DropForeignKey { table, .. } => Some(table.clone()),
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
        for (i, entry) in index.columns.iter().enumerate() {
            if i != 0 {
                sql.push_str(", ");
            }
            // SQLite has no operator classes, so one is omitted rather than rendered as an
            // expression it would parse as a column reference. The index it creates is the
            // same one it would create without it.
            match &entry.target {
                besu_schema::IndexTarget::Column(name) => sql.push_str(&self.quote(name)),
                besu_schema::IndexTarget::Expression(expression) => sql.push_str(expression),
            }
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
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`; `apply_with` notices the change and
        // rebuilds the table, which writes the foreign key inside the new `CREATE TABLE`.
        String::new()
    }

    fn drop_foreign_key(&self, _table: &str, _fk: &besu_schema::ForeignKey) -> String {
        // See `add_foreign_key`: `apply_with` rebuilds the table instead.
        String::new()
    }

    fn add_unique(&self, _table: &str, _unique: &besu_schema::Unique) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`; a unique constraint can only be
        // written inside `CREATE TABLE`, so `apply_with` rebuilds the table.
        String::new()
    }

    fn drop_unique(&self, _table: &str, _unique: &besu_schema::Unique) -> String {
        // See `add_unique`: `apply_with` rebuilds the table instead.
        String::new()
    }

    fn add_check(&self, _table: &str, _check: &Check) -> String {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`; a check can only be written inside
        // `CREATE TABLE`. `apply_with` notices the change and rebuilds the table, so this
        // statement is never the one that applies it.
        String::new()
    }

    fn drop_check(&self, _table: &str, _check: &Check) -> String {
        // See `add_check`: `apply_with` rebuilds the table instead.
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

/// Whether `text` may follow `DEFAULT` without being parenthesised.
///
/// That is a literal or one of SQLite's bare keywords; anything else is an expression, and an
/// expression default only parses inside parentheses.
pub(crate) fn is_simple_default(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "NULL" | "TRUE" | "FALSE" | "CURRENT_DATE" | "CURRENT_TIME" | "CURRENT_TIMESTAMP"
    ) {
        return true;
    }

    // A string literal (`'x'`) or a blob literal (`x'00'`).
    if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
        return true;
    }
    if text.len() >= 3 && (text.starts_with("x'") || text.starts_with("X'")) && text.ends_with('\'')
    {
        return true;
    }

    // A numeric literal, including its sign and an exponent.
    let bytes = text.as_bytes();
    let mut index = 0;
    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        index += 1;
    }
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
        digits += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
            index += 1;
        }
        let exponent = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent {
            return false;
        }
    }
    index == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use besu_schema::{IndexColumn, IndexKind, Unique};

    #[test]
    fn an_index_is_folded_to_what_sqlite_can_express() {
        let index = Index {
            name: "search".into(),
            columns: vec![
                IndexColumn::column("email").with_opclass("text_pattern_ops"),
                IndexColumn::expression("lower(name)"),
            ],
            unique: false,
            kind: IndexKind::Gin,
            predicate: Some("active".into()),
        };

        let folded = Ddl::normalise_index(&SQLiteDdl, &index);
        assert_eq!(folded.kind, IndexKind::Btree);
        assert_eq!(folded.predicate, None);
        assert_eq!(folded.columns[0].opclass, None);
        // Expression entries survive: SQLite understands them and introspection reads them.
        assert_eq!(folded.columns[1].as_expression(), Some("lower(name)"));
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
    fn a_unique_is_written_inside_create_table() {
        assert_eq!(
            SQLiteDdl.create_table(&table()),
            r#"CREATE TABLE "memberships" ("user_id" INTEGER NOT NULL, CONSTRAINT "memberships_user_key" UNIQUE ("user_id"))"#
        );
    }

    #[test]
    fn a_unique_cannot_be_added_or_dropped_after_the_fact() {
        // SQLite has no `ALTER TABLE ... ADD CONSTRAINT`, so the change itself renders as
        // nothing; `apply_with` is what applies it, by rebuilding the table.
        let unique = &table().uniques[0];
        assert!(SQLiteDdl.add_unique("memberships", unique).is_empty());
        assert!(SQLiteDdl.drop_unique("memberships", unique).is_empty());
    }

    /// A default is written bare only when SQLite parses it bare; everything else has to be
    /// parenthesised or the `CREATE TABLE` is a syntax error.
    #[test]
    fn only_a_literal_default_is_written_without_parentheses() {
        for literal in [
            "1",
            "-1",
            "+1",
            "2.5",
            ".5",
            "1.",
            "1e5",
            "-1.5e-3",
            "'x'",
            "'it''s'",
            "x'00'",
            "NULL",
            "true",
            "CURRENT_TIMESTAMP",
        ] {
            assert!(is_simple_default(literal), "`{literal}` is a literal");
        }

        for expression in ["1 + 1", "1+1", "datetime('now')", "now()", "", "0x10"] {
            assert!(
                !is_simple_default(expression),
                "`{expression}` is an expression"
            );
        }
    }

    /// `apply_with` is what turns a constraint the table cannot `ALTER` into a rebuild.
    #[test]
    fn a_constraint_change_rebuilds_the_table() {
        use besu_schema::{Change, Schema};

        let mut current = Schema::new();
        current.tables.push(table());
        let desired = current.clone();
        let unique = desired.tables[0].uniques[0].clone();
        let changes = [
            Change::DropForeignKey {
                table: "memberships".into(),
                foreign_key: besu_schema::ForeignKey {
                    name: "memberships_user_id_fkey".into(),
                    columns: vec!["user_id".into()],
                    references_table: "users".into(),
                    references_columns: vec!["id".into()],
                    on_delete: besu_schema::ReferenceAction::Default,
                    on_update: besu_schema::ReferenceAction::Default,
                },
            },
            Change::AddUnique {
                table: "memberships".into(),
                unique: unique.clone(),
            },
            Change::DropUnique {
                table: "memberships".into(),
                unique,
            },
        ];

        for change in changes {
            let statements =
                SQLiteDdl.apply_with(std::slice::from_ref(&change), &current, &desired);
            assert!(
                statements
                    .iter()
                    .any(|statement| statement.contains("__besu_tmp")),
                "{change:?} must rebuild the table: {statements:?}"
            );
        }
    }

    #[test]
    fn an_enum_is_text_and_converges_against_the_column_it_creates() {
        // SQLite has no enum. The label is stored in a `TEXT` column, and folding the
        // declared enum onto `Text` is what makes the next push a no-op.
        let declared = TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy", "sad"]));
        assert_eq!(Ddl::type_name(&SQLiteDdl, &declared), "TEXT");
        assert_eq!(Ddl::normalise_type(&SQLiteDdl, &declared), TypeDef::Text);
    }

    #[test]
    fn textual_types_fold_onto_text() {
        let ddl = SQLiteDdl;
        for ty in [
            TypeDef::Bit(5),
            TypeDef::VarBit(0),
            TypeDef::Interval,
            TypeDef::Inet,
            TypeDef::Cidr,
            TypeDef::Set(besu_core::EnumLabels::from_static(&["a"])),
        ] {
            assert_eq!(Ddl::normalise_type(&ddl, &ty), TypeDef::Text, "{ty:?}");
        }
        assert_eq!(
            Ddl::type_name(&ddl, &TypeDef::Bit(5)),
            "TEXT",
            "a bit string is stored as text, not as an integer"
        );
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Domain(besu_core::DomainDef::new("positive", &TypeDef::Int4))
            ),
            TypeDef::Int8
        );
    }

    #[test]
    fn a_range_and_a_vector_fold_onto_text_and_a_spatial_type_onto_a_blob() {
        // SQLite has no range, vector or spatial type, so the declaration has to fold onto
        // the storage that introspection reports: `TEXT` for the canonical text and `BLOB`
        // for the EWKB bytes.
        let ddl = SQLiteDdl;
        let range = TypeDef::Range(besu_core::types::TypeRef::owned(TypeDef::Int4));
        assert_eq!(Ddl::type_name(&ddl, &range), "TEXT");
        assert_eq!(Ddl::normalise_type(&ddl, &range), TypeDef::Text);

        let vector = TypeDef::Vector(3);
        assert_eq!(Ddl::type_name(&ddl, &vector), "TEXT");
        assert_eq!(Ddl::normalise_type(&ddl, &vector), TypeDef::Text);

        let point = TypeDef::Geometry {
            kind: besu_core::GeometryKind::Point,
            srid: Some(4326),
        };
        assert_eq!(Ddl::type_name(&ddl, &point), "BLOB");
        assert_eq!(Ddl::normalise_type(&ddl, &point), TypeDef::Bytes);
        let geography = TypeDef::Geography {
            kind: besu_core::GeometryKind::Geometry,
            srid: None,
        };
        assert_eq!(Ddl::type_name(&ddl, &geography), "BLOB");
        assert_eq!(Ddl::normalise_type(&ddl, &geography), TypeDef::Bytes);
    }

    #[test]
    fn an_expression_index_keeps_its_expression_and_drops_the_opclass() {
        // SQLite supports expression indexes but has no operator classes; adding one to the
        // statement would make it a different expression.
        let index = besu_schema::Index {
            name: "users_search_idx".into(),
            columns: vec![
                IndexColumn::column("email").with_opclass("text_pattern_ops"),
                IndexColumn::expression("lower(email)"),
            ],
            unique: false,
            kind: IndexKind::Btree,
            predicate: None,
        };
        assert_eq!(
            SQLiteDdl.create_index("users", &index),
            r#"CREATE INDEX "users_search_idx" ON "users" ("email", lower(email))"#
        );
    }
}
