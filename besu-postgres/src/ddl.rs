use std::collections::{BTreeMap, BTreeSet};

use besu_core::{Dialect, TypeDef};
use besu_schema::{
    Change, Column, Ddl, Domain, Enum, ForeignKey, Index, Schema, Sequence, View,
    split_qualified_name, view_body,
};

use crate::Postgres;

/// DDL generation for PostgreSQL.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresDdl;

impl Ddl for PostgresDdl {
    fn name(&self) -> &'static str {
        "postgres"
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
        Postgres::type_name(ty.clone()).into_owned()
    }

    fn normalise_type(&self, ty: &TypeDef) -> TypeDef {
        // PostgreSQL has no unsigned integers, so each `UInt` is stored as the signed type
        // that holds it. Folding them here is what makes a schema declaring `u16` converge
        // against the `INTEGER` the database reports back.
        match ty {
            TypeDef::UInt(8) => TypeDef::Int2,
            TypeDef::UInt(16) => TypeDef::Int4,
            TypeDef::UInt(32) => TypeDef::Int8,
            // `u64` lives in `NUMERIC(20,0)`, which is indistinguishable from a
            // `Decimal(20, 0)`; folding both together lets either converge.
            TypeDef::UInt(64) => TypeDef::Decimal(20, 0),
            // The same is true of `u128` in `NUMERIC(39,0)`. A signed `i128` is declared as
            // `Decimal(39, 0)` already, so the two share a type and converge.
            TypeDef::UInt(128) => TypeDef::Decimal(39, 0),
            // The fold has to reach through the wrappers: `Array(UInt(16))` is declared
            // `INTEGER[]`, which introspection reports as `Array(Int4)`.
            TypeDef::Array(inner) => TypeDef::Array(besu_core::types::TypeRef::owned(
                self.normalise_type(inner.as_type()),
            )),
            // A range's subtype is folded the same way, and a subtype with no built-in range
            // type falls back to `TEXT` exactly as `type_name` renders it. The precision of a
            // `Decimal` subtype is not part of `numrange`, so it folds away too.
            TypeDef::Range(inner) => {
                let inner = match self.normalise_type(inner.as_type()) {
                    TypeDef::Decimal(_, _) => TypeDef::Decimal(0, 0),
                    other => other,
                };
                match crate::dialect::range_type_name(&inner) {
                    Some(_) => TypeDef::Range(besu_core::types::TypeRef::owned(inner)),
                    None => TypeDef::Text,
                }
            }
            TypeDef::Collated(inner, collation) => {
                TypeDef::Collated(Box::new(self.normalise_type(inner)), collation.clone())
            }
            // A MySQL `SET` is a text label list here.
            TypeDef::Set(_) => TypeDef::Text,
            // A domain's base type is folded like any other type, so `Domain(UInt(16))` in a
            // snapshot compares equal to the `Domain(Int4)` introspection reports.
            TypeDef::Domain(def) => TypeDef::Domain(besu_core::DomainDef {
                name: def.name.clone(),
                base: besu_core::types::TypeRef::owned(self.normalise_type(def.base.as_type())),
            }),
            // PostGIS reports a geometry with SRID zero as one with no SRID at all, so
            // `Some(0)` folds away exactly as MySQL's does.
            TypeDef::Geometry { kind, srid } => TypeDef::Geometry {
                kind: kind.clone(),
                srid: srid.filter(|srid| *srid != 0),
            },
            TypeDef::Geography { kind, srid } => TypeDef::Geography {
                kind: kind.clone(),
                srid: srid.filter(|srid| *srid != 0),
            },
            // A named enum folds onto the `Custom(name)` a column of that type is introspected
            // as, so a declaration converges against the server's own report. An enum with
            // no name has no type for PostgreSQL to create, and renders as `TEXT`.
            TypeDef::Enum(def) if def.name.is_empty() => TypeDef::Text,
            TypeDef::Enum(def) => TypeDef::Custom(def.name.clone()),
            other => other.clone(),
        }
    }

    /// PostgreSQL has no MySQL-style prefix keys. Keep operator classes and partial
    /// indexes, which it does support, but fold the prefix away before comparison.
    fn normalise_index(&self, index: &Index) -> Index {
        let mut index = index.clone();
        for column in &mut index.columns {
            column.prefix = None;
        }
        index
    }

    fn column_sql(&self, column: &Column, inline_pk: bool) -> String {
        let mut sql = format!(
            "{} {}",
            self.quote(&column.name),
            self.type_name(&column.ty)
        );

        if let Some(generated) = &column.generated {
            // PostgreSQL only implements stored generated columns, so `VIRTUAL` is created
            // as `STORED`; the alternative would be a statement the server rejects.
            sql.push_str(" GENERATED ALWAYS AS (");
            sql.push_str(&generated.expression);
            sql.push_str(") STORED");
        } else if column.auto_increment {
            // PostgreSQL's identity columns are the closest thing to a portable
            // `AUTO_INCREMENT` and, unlike `SERIAL`, do not add a hidden default.
            sql.push_str(" GENERATED BY DEFAULT AS IDENTITY");
        }

        if inline_pk && column.primary_key {
            sql.push_str(" PRIMARY KEY");
        }

        // A primary key is implicitly `NOT NULL`, so writing it again is noise.
        if !column.nullable && !column.primary_key {
            sql.push_str(" NOT NULL");
        }

        if column.unique && column.generated.is_none() {
            sql.push_str(" UNIQUE");
        }

        // A generated column's value comes from its expression, and PostgreSQL rejects a
        // default beside one.
        if column.generated.is_none()
            && let Some(default) = &column.default
        {
            sql.push_str(" DEFAULT ");
            sql.push_str(default);
        }

        sql
    }

    /// PostgreSQL can change a column in place, so only the parts that differ are emitted.
    ///
    /// Primary keys are changed as table constraints. Single-column `UNIQUE` is changed the
    /// same way. A generated column's expression cannot be altered portably, so the column
    /// is dropped and added.
    fn alter_column(&self, table: &str, from: &Column, to: &Column) -> Vec<String> {
        if from.generated != to.generated {
            return vec![
                self.drop_column(table, &from.name),
                self.add_column(table, to),
            ];
        }

        let table = self.quote_table(table);
        let column = self.quote(&to.name);
        let mut statements = Vec::new();

        if from.ty != to.ty {
            statements.push(format!(
                "ALTER TABLE {table} ALTER COLUMN {column} TYPE {}",
                self.type_name(&to.ty)
            ));
        }

        if from.nullable != to.nullable {
            if to.nullable {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP NOT NULL"
                ));
            } else {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} SET NOT NULL"
                ));
            }
        }

        if from.default != to.default {
            match &to.default {
                Some(default) => statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} SET DEFAULT {default}"
                )),
                None => statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP DEFAULT"
                )),
            }
        }

        if from.auto_increment != to.auto_increment {
            if to.auto_increment {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} \
                     ADD GENERATED BY DEFAULT AS IDENTITY"
                ));
            } else {
                statements.push(format!(
                    "ALTER TABLE {table} ALTER COLUMN {column} DROP IDENTITY"
                ));
            }
        }

        if from.unique != to.unique {
            if to.unique {
                statements.push(format!("ALTER TABLE {table} ADD UNIQUE ({column})"));
            } else {
                statements.push(drop_column_unique(&table, &to.name));
            }
        }

        if from.primary_key != to.primary_key {
            if to.primary_key {
                statements.push(format!("ALTER TABLE {table} ADD PRIMARY KEY ({column})"));
            } else {
                statements.push(drop_primary_key(&table));
            }
        }

        statements
    }

    fn apply_with(&self, changes: &[Change], current: &Schema, desired: &Schema) -> Vec<String> {
        // Primary-key membership can change through AlterColumn, AddColumn or DropColumn.
        // Compare the complete table definitions for every table change and rebuild each
        // differing key once.
        let mut keyed_tables = BTreeSet::new();
        for change in changes {
            let Some(table) = change.table() else {
                continue;
            };
            let (Some(current), Some(desired)) = (current.get(&table), desired.get(&table)) else {
                continue;
            };
            let current_key: Vec<&str> = current
                .primary_key()
                .into_iter()
                .map(|column| column.name.as_str())
                .collect();
            let desired_key: Vec<&str> = desired
                .primary_key()
                .into_iter()
                .map(|column| column.name.as_str())
                .collect();
            if current_key != desired_key {
                keyed_tables.insert(table);
            }
        }

        let mut pending_keys = BTreeMap::new();
        for table in &keyed_tables {
            let (Some(current), Some(desired)) = (current.get(table), desired.get(table)) else {
                continue;
            };
            let mut statements = Vec::new();
            if current.has_primary_key() {
                statements.push(drop_primary_key(&self.quote_table(table)));
            }
            let key = desired.primary_key();
            if !key.is_empty() {
                let columns = key
                    .iter()
                    .map(|column| self.quote(&column.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                statements.push(format!(
                    "ALTER TABLE {} ADD PRIMARY KEY ({columns})",
                    self.quote_table(table)
                ));
            }
            pending_keys.insert(table.clone(), statements);
        }

        // A referenced key cannot be dropped until its foreign keys are gone. Count the
        // drops per table and flush the rebuilt key immediately after the last one.
        let mut remaining_foreign_key_drops: BTreeMap<String, usize> = BTreeMap::new();
        for change in changes {
            if let Change::DropForeignKey { foreign_key, .. } = change {
                *remaining_foreign_key_drops
                    .entry(foreign_key.references_table.clone())
                    .or_default() += 1;
            }
        }

        fn flush_key(
            table: &str,
            statements: &mut Vec<String>,
            pending: &mut BTreeMap<String, Vec<String>>,
            flushed: &mut BTreeSet<String>,
        ) {
            if flushed.insert(table.to_owned())
                && let Some(key) = pending.remove(table)
            {
                statements.extend(key);
            }
        }

        let mut flushed_keys = BTreeSet::new();
        let mut statements = Vec::new();
        for change in changes {
            if let Change::DropForeignKey { foreign_key, .. } = change {
                statements.extend(self.change(change));
                let referenced = &foreign_key.references_table;
                if pending_keys.contains_key(referenced) {
                    let remaining = remaining_foreign_key_drops
                        .get_mut(referenced)
                        .expect("foreign-key drops were counted");
                    *remaining -= 1;
                    if *remaining == 0 {
                        flush_key(
                            referenced,
                            &mut statements,
                            &mut pending_keys,
                            &mut flushed_keys,
                        );
                    }
                }
                continue;
            }

            // New foreign keys need the desired key on their referenced table.
            if let Change::AddForeignKey { foreign_key, .. } = change {
                flush_key(
                    &foreign_key.references_table,
                    &mut statements,
                    &mut pending_keys,
                    &mut flushed_keys,
                );
            }
            // Dropping an old key column needs the old constraint gone first.
            if matches!(change, Change::DropColumn { .. } | Change::DropTable { .. })
                && let Some(table) = change.table()
            {
                flush_key(
                    &table,
                    &mut statements,
                    &mut pending_keys,
                    &mut flushed_keys,
                );
            }

            // Preserve the base implementation's kind-aware replacement: PostgreSQL cannot
            // turn a plain view into a materialised one (or the reverse) in place.
            if let Change::ReplaceView(view) = change
                && let Some(existing) = current.view(&view.name)
                && existing.materialised != view.materialised
            {
                statements.extend(self.drop_view(&existing.name, existing.materialised));
                statements.extend(self.create_view(view));
                continue;
            }

            let Change::AlterColumn { table, .. } = change else {
                statements.extend(self.change(change));
                continue;
            };
            if keyed_tables.contains(table) {
                // The complete key was handled above; render only the column's other edits.
                let mut change = change.clone();
                if let Change::AlterColumn { from, to, .. } = &mut change {
                    from.primary_key = false;
                    to.primary_key = false;
                }
                statements.extend(self.change(&change));
            } else {
                statements.extend(self.change(change));
            }
        }
        for table in &keyed_tables {
            flush_key(table, &mut statements, &mut pending_keys, &mut flushed_keys);
        }
        statements
    }

    /// PostgreSQL requires parentheses around an index expression unless it has the form of
    /// a function call. Wrapping an arithmetic expression is what makes it a statement the
    /// server accepts; a function call is left as written, which is also the spelling
    /// introspection reports back.
    fn index_expression_sql(&self, expression: &str) -> String {
        let expression = expression.trim();
        if expression.starts_with('(') || is_function_call(expression) {
            expression.to_owned()
        } else {
            format!("({expression})")
        }
    }

    fn drop_index(&self, table: &str, index: &Index) -> String {
        // An index belongs to the table's schema, so an unqualified `DROP INDEX` names the
        // wrong one whenever the table is not in the current schema.
        match split_qualified_name(table).as_slice() {
            [schema, _] => format!(
                "DROP INDEX {}.{}",
                self.quote(schema),
                self.quote(&index.name)
            ),
            _ => format!("DROP INDEX {}", self.quote(&index.name)),
        }
    }

    fn drop_foreign_key(&self, table: &str, fk: &ForeignKey) -> String {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            self.quote_table(table),
            self.quote(&fk.name)
        )
    }

    /// `CREATE TYPE ... AS ENUM (...)`.
    fn create_enum(&self, e: &Enum) -> Vec<String> {
        let values: Vec<String> = e
            .values
            .iter()
            .map(|value| format!("'{}'", value.replace('\'', "''")))
            .collect();
        vec![format!(
            "CREATE TYPE {} AS ENUM ({})",
            self.quote_table(&e.name),
            values.join(", ")
        )]
    }

    /// `DROP TYPE`.
    fn drop_enum(&self, name: &str) -> Vec<String> {
        vec![format!("DROP TYPE {}", self.quote_table(name))]
    }

    /// PostgreSQL is the only dialect with domains.
    fn expresses_domains(&self) -> bool {
        true
    }

    /// PostgreSQL is the only shipped dialect with standalone sequences.
    fn expresses_sequences(&self) -> bool {
        true
    }

    /// `CREATE DOMAIN name AS base_type`.
    ///
    /// The constraints and default a domain can carry are not part of the IR, so they are not
    /// written here: a domain that already exists is left exactly as it is, and one created
    /// from a snapshot is built from its name and base type alone.
    fn create_domain(&self, domain: &Domain) -> Vec<String> {
        vec![format!(
            "CREATE DOMAIN {} AS {}",
            self.quote_table(&domain.name),
            self.type_name(&domain.base)
        )]
    }

    /// `DROP DOMAIN`.
    fn drop_domain(&self, name: &str) -> Vec<String> {
        vec![format!("DROP DOMAIN {}", self.quote_table(name))]
    }

    /// `ALTER TYPE ... ADD VALUE`.
    ///
    /// Before PostgreSQL 12 this cannot run inside a transaction block. `push` executes
    /// statements one at a time, so it works there; a migration file that wraps several
    /// statements in a transaction may need `ALTER TYPE` in its own migration.
    fn add_enum_value(&self, name: &str, value: &str) -> Vec<String> {
        vec![format!(
            "ALTER TYPE {} ADD VALUE '{}'",
            self.quote_table(name),
            value.replace('\'', "''")
        )]
    }

    /// Replace an enum with one whose labels are in the desired order.
    ///
    /// PostgreSQL has no direct reorder operation. A temporary type is created in the same
    /// schema, table columns (including arrays of the enum) are converted through text, and
    /// the old type is dropped before the temporary one takes its name. Unsupported direct
    /// dependencies, such as domains or defaults that name the enum, fail explicitly rather
    /// than leaving a partially applied change.
    fn reorder_enum(&self, from: &Enum, to: &Enum) -> Vec<String> {
        let values = to
            .values
            .iter()
            .map(|value| format!("'{}'", value.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let old_type = self.quote_table(&from.name);
        let old_type_literal = escape_string_literal(&old_type);
        let (_, name) = from
            .name
            .rsplit_once('.')
            .unwrap_or(("", from.name.as_str()));
        let suffix = "__besu_reorder";
        let mut temporary = name.to_owned();
        while temporary.len() > 63 - suffix.len() {
            temporary.pop();
        }
        temporary.push_str(suffix);
        let temporary_name = escape_string_literal(&temporary);
        let values = escape_string_literal(&values);

        vec![format!(
            "DO $besu$\n\
             DECLARE\n\
               old_type regtype := {old_type_literal}::pg_catalog.regtype;\n\
               old_schema text;\n\
               old_name text;\n\
               temporary_name constant text := {temporary_name};\n\
               temporary_type regtype;\n\
               dependency record;\n\
             BEGIN\n\
               SELECT n.nspname, t.typname INTO old_schema, old_name\n\
               FROM pg_catalog.pg_type t\n\
               JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace\n\
               WHERE t.oid = old_type;\n\
\n\
               IF EXISTS (\n\
                 SELECT 1 FROM pg_catalog.pg_depend d\n\
                 WHERE d.refclassid = 'pg_type'::regclass\n\
                   AND d.refobjid = old_type\n\
                   AND d.deptype = 'n'\n\
                   AND NOT (d.classid = 'pg_class'::regclass AND d.objsubid > 0)\n\
               ) THEN\n\
                 RAISE EXCEPTION 'enum % has an unsupported dependency; use an explicit migration', old_type;\n\
               END IF;\n\
\n\
               EXECUTE pg_catalog.format(\n\
                 'CREATE TYPE %I.%I AS ENUM (%s)', old_schema, temporary_name, {values}\n\
               );\n\
               SELECT pg_catalog.format('%I.%I', old_schema, temporary_name)::regtype\n\
               INTO temporary_type;\n\
\n\
               FOR dependency IN\n\
                 SELECT n.nspname AS schema_name, c.relname AS table_name,\n\
                        a.attname AS column_name, t.oid = old_type AS direct,\n\
                        t.typelem = old_type AS array_element\n\
                 FROM pg_catalog.pg_depend d\n\
                 JOIN pg_catalog.pg_class c ON c.oid = d.objid\n\
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace\n\
                 JOIN pg_catalog.pg_attribute a\n\
                   ON a.attrelid = c.oid AND a.attnum = d.objsubid\n\
                 JOIN pg_catalog.pg_type t ON t.oid = a.atttypid\n\
                 WHERE d.classid = 'pg_class'::regclass\n\
                   AND d.refclassid = 'pg_type'::regclass\n\
                   AND d.refobjid = old_type\n\
                   AND d.deptype = 'n'\n\
                   AND (t.oid = old_type OR t.typelem = old_type)\n\
               LOOP\n\
                 IF dependency.direct THEN\n\
                   EXECUTE pg_catalog.format(\n\
                     'ALTER TABLE ONLY %I.%I ALTER COLUMN %I TYPE %s USING %I::text::%s',\n\
                     dependency.schema_name, dependency.table_name, dependency.column_name,\n\
                     temporary_type, dependency.column_name, temporary_type\n\
                   );\n\
                 ELSE\n\
                   EXECUTE pg_catalog.format(\n\
                     'ALTER TABLE ONLY %I.%I ALTER COLUMN %I TYPE %s[] USING %I::text[]::%s[]',\n\
                     dependency.schema_name, dependency.table_name, dependency.column_name,\n\
                     temporary_type, dependency.column_name, temporary_type\n\
                   );\n\
                 END IF;\n\
               END LOOP;\n\
\n\
               EXECUTE pg_catalog.format('DROP TYPE %s', old_type);\n\
               EXECUTE pg_catalog.format(\n\
                 'ALTER TYPE %I.%I RENAME TO %I', old_schema, temporary_name, old_name\n\
               );\n\
             END\n\
             $besu$"
        )]
    }

    /// `CREATE [MATERIALIZED] VIEW`.
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

    /// Replace a view, recreating a materialised one.
    ///
    /// PostgreSQL has no `CREATE OR REPLACE MATERIALIZED VIEW`, so a changed materialised
    /// view is dropped and created again. That re-runs its query, which is why the change is
    /// reported as `Risky`.
    fn replace_view(&self, view: &View) -> Vec<String> {
        if view.materialised {
            let mut statements = self.drop_view(&view.name, true);
            statements.extend(self.create_view(view));
            statements.extend(self.refresh_view(view));
            return statements;
        }
        vec![format!(
            "CREATE OR REPLACE VIEW {} AS {}",
            self.quote_table(&view.name),
            view_body(&view.definition)
        )]
    }

    /// `DROP [MATERIALIZED] VIEW`.
    fn drop_view(&self, name: &str, materialised: bool) -> Vec<String> {
        let materialised = if materialised { "MATERIALIZED " } else { "" };
        vec![format!(
            "DROP {materialised}VIEW {}",
            self.quote_table(name)
        )]
    }

    /// `REFRESH MATERIALIZED VIEW`.
    fn refresh_view(&self, view: &View) -> Vec<String> {
        if !view.materialised {
            return Vec::new();
        }
        vec![format!(
            "REFRESH MATERIALIZED VIEW {}",
            self.quote_table(&view.name)
        )]
    }

    /// `CREATE SEQUENCE`.
    ///
    /// The options are spelled out so the generated statement describes exactly the sequence
    /// the IR holds; a bound of `None` is left to PostgreSQL's default for the data type.
    fn create_sequence(&self, sequence: &Sequence) -> Vec<String> {
        let mut sql = format!(
            "CREATE SEQUENCE {} AS {}",
            self.quote_table(&sequence.qualified_name()),
            sequence.data_type
        );
        sql.push_str(&format!(" INCREMENT BY {}", sequence.increment));
        if let Some(min) = sequence.min {
            sql.push_str(&format!(" MINVALUE {min}"));
        }
        if let Some(max) = sequence.max {
            sql.push_str(&format!(" MAXVALUE {max}"));
        }
        sql.push_str(&format!(" START WITH {}", sequence.start));
        sql.push_str(&format!(" CACHE {}", sequence.cache));
        sql.push_str(if sequence.cycle {
            " CYCLE"
        } else {
            " NO CYCLE"
        });
        vec![sql]
    }

    /// `ALTER SEQUENCE`.
    ///
    /// Every option is written, including the ones being reset to their default: there is no
    /// partial form that converges once a bound has been narrowed, so the statement describes
    /// the whole desired sequence.
    fn alter_sequence(&self, _from: &Sequence, to: &Sequence) -> Vec<String> {
        let mut sql = format!(
            "ALTER SEQUENCE {} AS {}",
            self.quote_table(&to.qualified_name()),
            to.data_type
        );
        sql.push_str(&format!(" INCREMENT BY {}", to.increment));
        match to.min {
            Some(min) => sql.push_str(&format!(" MINVALUE {min}")),
            None => sql.push_str(" NO MINVALUE"),
        }
        match to.max {
            Some(max) => sql.push_str(&format!(" MAXVALUE {max}")),
            None => sql.push_str(" NO MAXVALUE"),
        }
        sql.push_str(&format!(" START WITH {}", to.start));
        sql.push_str(&format!(" CACHE {}", to.cache));
        sql.push_str(if to.cycle { " CYCLE" } else { " NO CYCLE" });
        vec![sql]
    }

    /// `DROP SEQUENCE`.
    fn drop_sequence(&self, name: &str) -> Vec<String> {
        vec![format!("DROP SEQUENCE {}", self.quote_table(name))]
    }
}

fn drop_column_unique(table: &str, column: &str) -> String {
    // The column flag does not carry a name. Match the exact set of constrained columns
    // under the same lock used to drop the constraint, and never guess if it is ambiguous.
    let relation = escape_string_literal(table);
    let column = escape_string_literal(column);
    let body = format!(
        "DECLARE
            constraint_name text;
         BEGIN
            LOCK TABLE ONLY {table} IN ACCESS EXCLUSIVE MODE;
            SELECT pg_catalog.min(c.conname::text) INTO constraint_name
            FROM pg_catalog.pg_constraint c
            JOIN pg_catalog.pg_attribute a ON a.attrelid = c.conrelid
            WHERE c.conrelid = {relation}::pg_catalog.regclass
              AND c.contype = 'u' AND c.conkey = ARRAY[a.attnum] AND a.attname = {column}
            HAVING count(*) = 1 AND pg_catalog.bool_and(c.conparentid = 0 AND c.coninhcount = 0);
            IF constraint_name IS NULL THEN
                RAISE SQLSTATE '0A000' USING MESSAGE =
                    'cannot identify a single local UNIQUE constraint for the column; use an explicit migration';
            END IF;
            EXECUTE pg_catalog.format('ALTER TABLE ONLY %s DROP CONSTRAINT %I', {relation}, constraint_name);
         END;"
    );
    format!("DO {}", escape_string_literal(&body))
}

fn drop_primary_key(table: &str) -> String {
    // The column flags do not carry the constraint's name, and callers may have chosen a
    // custom one. Find the table's single local primary key under the same lock used to drop
    // it rather than guessing PostgreSQL's usual `<table>_pkey` spelling.
    let relation = escape_string_literal(table);
    let body = format!(
        "DECLARE
            constraint_name text;
         BEGIN
            LOCK TABLE ONLY {table} IN ACCESS EXCLUSIVE MODE;
            SELECT pg_catalog.min(c.conname::text) INTO constraint_name
            FROM pg_catalog.pg_constraint c
            WHERE c.conrelid = {relation}::pg_catalog.regclass
              AND c.contype = 'p'
            HAVING count(*) = 1 AND pg_catalog.bool_and(c.conparentid = 0 AND c.coninhcount = 0);
            IF constraint_name IS NULL THEN
                RAISE SQLSTATE '0A000' USING MESSAGE =
                    'cannot identify a single local PRIMARY KEY constraint for the table; use an explicit migration';
            END IF;
            EXECUTE pg_catalog.format('ALTER TABLE ONLY %s DROP CONSTRAINT %I', {relation}, constraint_name);
         END;"
    );
    format!("DO {}", escape_string_literal(&body))
}

fn escape_string_literal(text: &str) -> String {
    format!("E'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
}

/// Whether an expression has the form `name(...)`, which PostgreSQL accepts as an index key
/// without an extra pair of parentheses.
///
/// The name may be schema-qualified and quoted. Anything else — arithmetic, a cast, a
/// literal — needs the parentheses.
fn is_function_call(expression: &str) -> bool {
    let Some(open) = expression.find('(') else {
        return false;
    };
    let head = expression[..open].trim();
    !head.is_empty()
        && head
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '"' | '$'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use besu_schema::{IndexColumn, IndexKind, View, diff_with};

    #[test]
    fn a_named_enum_renders_its_name_and_an_unnamed_one_is_text() {
        // PostgreSQL names the type, so the column's declaration is the type's name. An
        // inline enum from another dialect has nothing to create here and reads as text.
        assert_eq!(
            Postgres::type_name(TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy"]))),
            r#""mood""#
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Enum(besu_core::EnumDef::new("", &["happy"]))),
            "TEXT"
        );
    }

    #[test]
    fn reordering_an_enum_renders_a_type_replacement() {
        let from = Enum {
            name: "mood".into(),
            values: vec!["happy".into(), "sad".into()],
        };
        let to = Enum {
            name: "mood".into(),
            values: vec!["sad".into(), "happy".into()],
        };
        let statements = PostgresDdl.reorder_enum(&from, &to);
        assert_eq!(statements.len(), 1);
        let statement = &statements[0];
        assert!(statement.starts_with("DO $besu$"), "{statement}");
        assert!(
            statement.contains("CREATE TYPE %I.%I AS ENUM (%s)"),
            "{statement}"
        );
        assert!(statement.contains("E'''sad'', ''happy'''"), "{statement}");
        assert!(statement.contains("DROP TYPE %s"), "{statement}");
        assert!(statement.contains("RENAME TO %I"), "{statement}");
    }

    #[test]
    fn a_primary_key_only_change_renders_ddl() {
        fn column(name: &str, primary_key: bool) -> Column {
            Column {
                name: name.into(),
                ty: TypeDef::Int8,
                nullable: false,
                auto_increment: false,
                primary_key,
                unique: false,
                default: None,
                generated: None,
                comment: None,
                renamed_from: None,
            }
        }
        fn table(columns: Vec<Column>) -> besu_schema::Table {
            besu_schema::Table {
                name: "things".into(),
                schema: None,
                columns,
                indexes: Vec::new(),
                uniques: Vec::new(),
                foreign_keys: Vec::new(),
                checks: Vec::new(),
                strict: false,
                without_rowid: false,
                comment: None,
            }
        }

        let plain = column("id", false);
        let keyed = column("id", true);
        assert_eq!(
            PostgresDdl.change(&Change::AlterColumn {
                table: "things".into(),
                from: plain.clone(),
                to: keyed.clone(),
            }),
            ["ALTER TABLE \"things\" ADD PRIMARY KEY (\"id\")"]
        );
        let dropped = PostgresDdl.change(&Change::AlterColumn {
            table: "things".into(),
            from: keyed,
            to: plain,
        });
        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].contains("PRIMARY KEY constraint"), "{dropped:?}");

        // Replacing a composite key is one drop and one add, not an add per newly keyed
        // column while the old constraint still exists.
        let current = Schema {
            tables: vec![table(vec![
                column("a", true),
                column("b", true),
                column("c", false),
            ])],
            ..Schema::default()
        };
        let desired = Schema {
            tables: vec![table(vec![
                column("a", true),
                column("b", false),
                column("c", true),
            ])],
            ..Schema::default()
        };
        let statements = PostgresDdl.apply_with(
            &[Change::AlterColumn {
                table: "things".into(),
                from: column("c", false),
                to: column("c", true),
            }],
            &current,
            &desired,
        );
        assert_eq!(statements.len(), 2, "{statements:?}");
        assert!(statements[0].contains("DROP CONSTRAINT"));
        assert_eq!(
            statements[1],
            "ALTER TABLE \"things\" ADD PRIMARY KEY (\"a\", \"c\")"
        );
    }

    #[test]
    fn primary_key_rebuilds_cover_column_membership_and_foreign_keys() {
        let column = |name: &str, primary_key| Column {
            name: name.into(),
            ty: TypeDef::Int8,
            nullable: false,
            auto_increment: false,
            primary_key,
            unique: false,
            default: None,
            generated: None,
            comment: None,
            renamed_from: None,
        };
        let table = |name: &str, columns: Vec<Column>| besu_schema::Table {
            name: name.into(),
            schema: None,
            columns,
            indexes: Vec::new(),
            uniques: Vec::new(),
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            strict: false,
            without_rowid: false,
            comment: None,
        };
        let apply = |current: Schema, desired: Schema| {
            let changes = diff_with(&current, &desired, &|ty| ty.clone());
            PostgresDdl.apply_with(&changes, &current, &desired)
        };

        // Adding a composite member is only an AddColumn, but the complete desired key must
        // still be built.
        let current = Schema {
            tables: vec![table("things", vec![column("a", true)])],
            ..Schema::default()
        };
        let desired = Schema {
            tables: vec![table("things", vec![column("a", true), column("b", true)])],
            ..Schema::default()
        };
        let statements = apply(current, desired);
        assert_eq!(statements.len(), 3, "{statements:?}");
        assert_eq!(
            statements[0],
            r#"ALTER TABLE "things" ADD COLUMN "b" BIGINT"#
        );
        assert!(statements[1].contains("PRIMARY KEY constraint"));
        assert_eq!(
            statements[2],
            r#"ALTER TABLE "things" ADD PRIMARY KEY ("a", "b")"#
        );

        // Dropping a composite member must drop the old complete key before the column.
        let current = Schema {
            tables: vec![table("things", vec![column("a", true), column("b", true)])],
            ..Schema::default()
        };
        let desired = Schema {
            tables: vec![table("things", vec![column("a", true)])],
            ..Schema::default()
        };
        let statements = apply(current, desired);
        assert_eq!(statements.len(), 3, "{statements:?}");
        assert!(statements[0].contains("PRIMARY KEY constraint"));
        assert_eq!(
            statements[1],
            r#"ALTER TABLE "things" ADD PRIMARY KEY ("a")"#
        );
        assert_eq!(statements[2], r#"ALTER TABLE "things" DROP COLUMN "b""#);

        // Replacing the sole key column is an add plus a drop. The new column must exist
        // before the new key is added, and the old constraint must go before its column.
        let current = Schema {
            tables: vec![table("things", vec![column("old", true)])],
            ..Schema::default()
        };
        let desired = Schema {
            tables: vec![table("things", vec![column("new", true)])],
            ..Schema::default()
        };
        let statements = apply(current, desired);
        assert_eq!(statements.len(), 4, "{statements:?}");
        assert_eq!(
            statements[0],
            r#"ALTER TABLE "things" ADD COLUMN "new" BIGINT"#
        );
        assert!(statements[1].contains("PRIMARY KEY constraint"));
        assert_eq!(
            statements[2],
            r#"ALTER TABLE "things" ADD PRIMARY KEY ("new")"#
        );
        assert_eq!(statements[3], r#"ALTER TABLE "things" DROP COLUMN "old""#);

        // A referenced key is rebuilt after dependent foreign keys are dropped and before
        // they are added again.
        let foreign_key = besu_schema::ForeignKey {
            name: "child_parent_fk".into(),
            columns: vec!["parent_id".into()],
            references_table: "parents".into(),
            references_columns: vec!["id".into()],
            on_delete: besu_schema::ReferenceAction::Default,
            on_update: besu_schema::ReferenceAction::Default,
        };
        let mut current_child = table("children", vec![column("parent_id", false)]);
        current_child.foreign_keys.push(foreign_key.clone());
        let current = Schema {
            tables: vec![
                table("parents", vec![column("id", true), column("code", false)]),
                current_child,
            ],
            ..Schema::default()
        };
        let mut desired_child = table("children", vec![column("parent_id", false)]);
        let mut retargeted = foreign_key;
        retargeted.references_columns = vec!["code".into()];
        desired_child.foreign_keys.push(retargeted);
        let desired = Schema {
            tables: vec![
                table("parents", vec![column("id", false), column("code", true)]),
                desired_child,
            ],
            ..Schema::default()
        };
        let statements = apply(current, desired);
        let drop_fk = statements
            .iter()
            .position(|statement| {
                statement == r#"ALTER TABLE "children" DROP CONSTRAINT "child_parent_fk""#
            })
            .expect("foreign key drop is planned");
        let drop_pk = statements
            .iter()
            .position(|statement| statement.contains("PRIMARY KEY constraint"))
            .expect("primary key drop is planned");
        let add_pk = statements
            .iter()
            .position(|statement| statement == r#"ALTER TABLE "parents" ADD PRIMARY KEY ("code")"#)
            .expect("new primary key is planned");
        let add_fk = statements
            .iter()
            .position(|statement| statement == r#"ALTER TABLE "children" ADD CONSTRAINT "child_parent_fk" FOREIGN KEY ("parent_id") REFERENCES "parents" ("code")"#)
            .expect("retargeted foreign key is planned");
        assert!(
            drop_fk < drop_pk && drop_pk < add_pk && add_pk < add_fk,
            "{statements:?}"
        );

        // A newly added foreign key also needs the referenced table's new key first, even
        // when there was no old foreign key to drop.
        let current = Schema {
            tables: vec![
                table("parents", vec![column("id", false), column("code", false)]),
                table("children", vec![column("parent_id", false)]),
            ],
            ..Schema::default()
        };
        let mut desired_child = table("children", vec![column("parent_id", false)]);
        desired_child.foreign_keys.push(besu_schema::ForeignKey {
            name: "child_parent_fk".into(),
            columns: vec!["parent_id".into()],
            references_table: "parents".into(),
            references_columns: vec!["code".into()],
            on_delete: besu_schema::ReferenceAction::Default,
            on_update: besu_schema::ReferenceAction::Default,
        });
        let desired = Schema {
            tables: vec![
                table("parents", vec![column("id", false), column("code", true)]),
                desired_child,
            ],
            ..Schema::default()
        };
        let statements = apply(current, desired);
        let add_pk = statements
            .iter()
            .position(|statement| statement == r#"ALTER TABLE "parents" ADD PRIMARY KEY ("code")"#)
            .expect("new primary key is planned");
        let add_fk = statements
            .iter()
            .position(|statement| statement.contains(r#"REFERENCES "parents" ("code")"#))
            .expect("new foreign key is planned");
        assert!(add_pk < add_fk, "{statements:?}");
    }

    #[test]
    fn changing_view_kind_drops_the_kind_that_exists() {
        let plain = Schema {
            views: vec![View::new("user_ids", "SELECT id FROM users")],
            ..Schema::default()
        };
        let mut materialised = plain.clone();
        materialised.views[0].materialised = true;
        let changes = diff_with(&plain, &materialised, &|ty| ty.clone());
        assert_eq!(
            PostgresDdl.apply_with(&changes, &plain, &materialised),
            [
                r#"DROP VIEW "user_ids""#,
                r#"CREATE MATERIALIZED VIEW "user_ids" AS SELECT id FROM users"#,
            ]
        );

        let changes = diff_with(&materialised, &plain, &|ty| ty.clone());
        assert_eq!(
            PostgresDdl.apply_with(&changes, &materialised, &plain),
            [
                r#"DROP MATERIALIZED VIEW "user_ids""#,
                r#"CREATE VIEW "user_ids" AS SELECT id FROM users"#,
            ]
        );
    }

    #[test]
    fn a_named_enum_folds_onto_the_custom_type_introspection_reports() {
        let ddl = PostgresDdl;
        let declared = TypeDef::Enum(besu_core::EnumDef::new("mood", &["happy", "sad"]));
        assert_eq!(
            Ddl::normalise_type(&ddl, &declared),
            TypeDef::Custom("mood".into())
        );
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Enum(besu_core::EnumDef::new("", &["happy", "sad"]))
            ),
            TypeDef::Text
        );
    }

    #[test]
    fn interval_network_and_bit_types_have_their_native_spellings() {
        assert_eq!(Postgres::type_name(TypeDef::Interval), "INTERVAL");
        assert_eq!(Postgres::type_name(TypeDef::Inet), "INET");
        assert_eq!(Postgres::type_name(TypeDef::Cidr), "CIDR");
        assert_eq!(Postgres::type_name(TypeDef::Bit(5)), "BIT(5)");
        assert_eq!(Postgres::type_name(TypeDef::VarBit(0)), "BIT VARYING");
        assert_eq!(Postgres::type_name(TypeDef::VarBit(7)), "BIT VARYING(7)");
        // MySQL's `SET` has no PostgreSQL spelling, so it is text and folds the same way.
        assert_eq!(
            Postgres::type_name(TypeDef::Set(besu_core::EnumLabels::from_static(&["a"]))),
            "TEXT"
        );
    }

    #[test]
    fn a_domain_renders_its_name_and_its_base_folds() {
        let ddl = PostgresDdl;
        let domain = TypeDef::Domain(besu_core::DomainDef::new("positive", &TypeDef::Int4));
        assert_eq!(Postgres::type_name(domain.clone()), r#""positive""#);
        assert_eq!(Ddl::normalise_type(&ddl, &domain), domain);

        // A snapshot declaring `u16` as the base of a domain has to converge against the
        // `integer` the database reports for it.
        let declared = TypeDef::Domain(besu_core::DomainDef::new("small", &TypeDef::UInt(16)));
        assert_eq!(
            Ddl::normalise_type(&ddl, &declared),
            TypeDef::Domain(besu_core::DomainDef::new("small", &TypeDef::Int4))
        );
    }

    #[test]
    fn ranges_and_spatial_columns_render_their_postgis_spellings() {
        use besu_core::GeometryKind;
        use besu_core::types::TypeRef;

        let range = |element: TypeDef| TypeDef::Range(TypeRef::owned(element));
        assert_eq!(Postgres::type_name(range(TypeDef::Int4)), "int4range");
        assert_eq!(Postgres::type_name(range(TypeDef::Int8)), "int8range");
        assert_eq!(
            Postgres::type_name(range(TypeDef::Decimal(10, 2))),
            "numrange",
            "numrange's subtype is an unconstrained numeric, whatever the marker says"
        );
        assert_eq!(Postgres::type_name(range(TypeDef::Timestamp)), "tsrange");
        assert_eq!(
            Postgres::type_name(range(TypeDef::TimestampTz)),
            "tstzrange"
        );
        assert_eq!(Postgres::type_name(range(TypeDef::Date)), "daterange");
        // A subtype PostgreSQL has no built-in range for is stored as text, exactly as the
        // fold says.
        assert_eq!(Postgres::type_name(range(TypeDef::Text)), "TEXT");
        assert_eq!(
            Postgres::type_name(TypeDef::Array(TypeRef::owned(range(TypeDef::Int4)))),
            "int4range[]"
        );

        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Point,
                srid: Some(4326),
            }),
            "GEOMETRY(POINT,4326)"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Geometry,
                srid: None,
            }),
            "GEOMETRY"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geography {
                kind: GeometryKind::MultiPolygon,
                srid: Some(3857),
            }),
            "GEOGRAPHY(MULTIPOLYGON,3857)"
        );
        assert_eq!(
            Postgres::type_name(TypeDef::Geometry {
                kind: GeometryKind::Other("PointZ".into()),
                srid: None,
            }),
            "GEOMETRY(PointZ)"
        );

        assert_eq!(Postgres::type_name(TypeDef::Vector(3)), "VECTOR(3)");
        assert_eq!(Postgres::type_name(TypeDef::Vector(0)), "VECTOR");
    }

    #[test]
    fn a_range_and_a_vector_declaration_fold_onto_what_introspection_reports() {
        use besu_core::types::TypeRef;

        let ddl = PostgresDdl;
        // `numrange`'s subtype is an unconstrained numeric, so the marker's precision and
        // scale cannot survive a push/pull and fold away.
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Range(TypeRef::owned(TypeDef::Decimal(10, 2)))
            ),
            TypeDef::Range(TypeRef::owned(TypeDef::Decimal(0, 0)))
        );
        // A range with no PostgreSQL spelling is stored as text.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Range(TypeRef::owned(TypeDef::Text))),
            TypeDef::Text
        );
        // The unsigned markers fold inside the subtype, the same as they do in an array.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Range(TypeRef::owned(TypeDef::UInt(32)))),
            TypeDef::Range(TypeRef::owned(TypeDef::Int8))
        );
        // PostGIS has no SRID zero: a column declared with one converges against the plain
        // shape introspection reports.
        assert_eq!(
            Ddl::normalise_type(
                &ddl,
                &TypeDef::Geometry {
                    kind: besu_core::GeometryKind::Point,
                    srid: Some(0),
                }
            ),
            TypeDef::Geometry {
                kind: besu_core::GeometryKind::Point,
                srid: None,
            }
        );
        // A vector's dimension is part of its type, and both are already what is reported.
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Vector(3)),
            TypeDef::Vector(3)
        );
        assert_eq!(
            Ddl::normalise_type(&ddl, &TypeDef::Vector(0)),
            TypeDef::Vector(0)
        );
    }

    #[test]
    fn a_domain_is_created_with_its_name_and_base() {
        let ddl = PostgresDdl;
        let domain = besu_schema::Domain {
            name: "positive".into(),
            base: TypeDef::Int4,
        };
        assert_eq!(
            Ddl::create_domain(&ddl, &domain),
            [r#"CREATE DOMAIN "positive" AS INTEGER"#]
        );
        assert_eq!(
            Ddl::drop_domain(&ddl, "positive"),
            [r#"DROP DOMAIN "positive""#]
        );
    }

    #[test]
    fn an_arithmetic_expression_is_parenthesised_and_a_function_call_is_not() {
        let index = Index {
            name: "events_score_idx".into(),
            columns: vec![
                IndexColumn::expression("a + b"),
                IndexColumn::expression("lower(name)"),
            ],
            unique: false,
            kind: IndexKind::Btree,
            predicate: None,
        };

        assert_eq!(
            PostgresDdl.create_index("events", &index),
            r#"CREATE INDEX "events_score_idx" ON "events" ((a + b), lower(name))"#
        );
    }
}
