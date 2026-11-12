use crate::{Check, Column, Enum, ForeignKey, Index, Schema, Sequence, Table, Unique, View};

/// How much damage applying a [`Change`] can do.
///
/// Anything other than [`Safety::Safe`] is reported by `besu push` and requires an explicit
/// flag, because these are the changes that lose data. The type system cannot protect you
/// from a `DROP COLUMN`, so this is the next best thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Safety {
    /// Lossless and cheap: creating a table or adding a nullable column.
    Safe,
    /// Reversible but rewrites data: changing a type, adding an index.
    Risky,
    /// Data loss: dropping a column, table or enum variant.
    Destructive,
}

/// One difference between two schemas.
///
/// Every variant that names a table carries the table's *qualified* name — `app.users` for
/// a table in a schema, and a bare `users` for one in the default schema — because the
/// identity of a table is `(schema, name)`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// The table exists only in the desired schema.
    CreateTable(Table),
    /// The table exists only in the current schema.
    DropTable {
        /// The qualified name of the table being removed.
        name: String,
    },
    /// A named enum type exists only in the desired schema.
    CreateEnum(Enum),
    /// A named enum type exists only in the current schema.
    DropEnum {
        /// The enum's name.
        name: String,
    },
    /// A value was appended to an existing enum type.
    AddEnumValue {
        /// The enum's name.
        name: String,
        /// The value being added.
        value: String,
    },
    /// An enum value is no longer declared.
    ///
    /// PostgreSQL has no `ALTER TYPE ... DROP VALUE`, so this cannot be rendered as DDL; it
    /// is reported so that it is never silently ignored.
    RemoveEnumValue {
        /// The enum's name.
        name: String,
        /// The value being removed.
        value: String,
    },
    /// A column was added.
    AddColumn {
        /// The table.
        table: String,
        /// The new column.
        column: Column,
    },
    /// A column was removed.
    DropColumn {
        /// The table.
        table: String,
        /// The removed column.
        column: String,
    },
    /// A column was renamed, keeping its data.
    RenameColumn {
        /// The table.
        table: String,
        /// The name it has now.
        from: String,
        /// The name it should have.
        to: String,
    },
    /// A column's definition changed.
    AlterColumn {
        /// The table.
        table: String,
        /// The column as it exists now.
        from: Column,
        /// The column as it should be.
        to: Column,
    },
    /// A table-level unique constraint is gone and must be re-created as an index.
    AddIndex {
        /// The table.
        table: String,
        /// The index.
        index: Index,
    },
    /// An index was removed.
    DropIndex {
        /// The table.
        table: String,
        /// The index.
        index: Index,
    },
    /// A multi-column `UNIQUE` constraint was added.
    AddUnique {
        /// The table.
        table: String,
        /// The constraint.
        unique: Unique,
    },
    /// A multi-column `UNIQUE` constraint was removed.
    DropUnique {
        /// The table.
        table: String,
        /// The constraint.
        unique: Unique,
    },
    /// A foreign key was added.
    AddForeignKey {
        /// The table.
        table: String,
        /// The constraint.
        foreign_key: ForeignKey,
    },
    /// A foreign key was removed.
    DropForeignKey {
        /// The table.
        table: String,
        /// The constraint.
        foreign_key: ForeignKey,
    },
    /// A `CHECK` constraint was added.
    AddCheck {
        /// The table.
        table: String,
        /// The constraint.
        check: Check,
    },
    /// A `CHECK` constraint was removed.
    DropCheck {
        /// The table.
        table: String,
        /// The constraint.
        check: Check,
    },
    /// A view exists only in the desired schema.
    CreateView(View),
    /// A view exists on both sides but its definition changed.
    ///
    /// Creating the view again is a `Risky` change rather than a destructive one: the
    /// existing view is replaced, so nothing it reads is lost. Replacing a materialised view
    /// re-runs its query, which can be expensive but is still not destructive.
    ReplaceView(View),
    /// A view exists only in the current schema.
    DropView {
        /// The view's (possibly qualified) name.
        name: String,
        /// Whether it was a materialised view, which needs its own `DROP` spelling.
        materialised: bool,
    },
    /// A standalone sequence exists only in the desired schema.
    CreateSequence(Sequence),
    /// A sequence exists on both sides but one of its options changed.
    AlterSequence {
        /// The qualified sequence name.
        name: String,
        /// The sequence as it exists now.
        from: Sequence,
        /// The sequence as it should be.
        to: Sequence,
    },
    /// A sequence exists only in the current schema.
    DropSequence {
        /// The qualified sequence name.
        name: String,
    },
    /// Row-level security should be enabled on a table.
    ///
    /// There is deliberately no inverse: disabling RLS silently removes the guarantee every
    /// policy on the table depends on, so it is left to an explicit migration.
    EnableRls {
        /// The table.
        table: String,
    },
}

impl Change {
    /// How dangerous this change is.
    pub fn safety(&self) -> Safety {
        match self {
            Self::CreateTable(_) | Self::CreateEnum(_) | Self::CreateSequence(_) => Safety::Safe,
            // A view is only a stored query, so creating one cannot touch a row.
            Self::CreateView(_) | Self::EnableRls { .. } => Safety::Safe,
            // A rename rewrites metadata, and can break views or code that named the old
            // column. It never loses the column's data, so it is not destructive.
            Self::RenameColumn { .. } => Safety::Risky,
            Self::AddColumn { column, .. } => {
                if column.nullable || column.default.is_some() || column.generated.is_some() {
                    // A generated column has no value of its own to invent, so adding one
                    // cannot fail the way a `NOT NULL` column with no default does.
                    Safety::Safe
                } else {
                    // Adding a NOT NULL column without a default fails on a non-empty table.
                    Safety::Risky
                }
            }
            Self::AddIndex { .. }
            | Self::AddUnique { .. }
            | Self::AddForeignKey { .. }
            | Self::AddCheck { .. }
            // Appending an enum value cannot break an existing row; it can only be slow.
            | Self::AddEnumValue { .. } => Safety::Risky,
            // Replacing a view re-runs its query, and for a materialised view that query
            // runs immediately.
            Self::ReplaceView(_) => Safety::Risky,
            // Changing a sequence's bounds or type can make `nextval` fail for values that
            // are already handed out, and rewrites the catalogue.
            Self::AlterSequence { .. } => Safety::Risky,
            Self::AlterColumn { from, to, .. } => {
                if from.ty == to.ty
                    && from.nullable == to.nullable
                    && from.default == to.default
                    && from.auto_increment == to.auto_increment
                    && from.generated == to.generated
                {
                    Safety::Safe
                } else {
                    Safety::Risky
                }
            }
            Self::DropTable { .. }
            | Self::DropColumn { .. }
            | Self::DropIndex { .. }
            // Losing a unique constraint loses the guarantee, exactly like dropping a unique
            // index, and it is the only way to reach the desired schema.
            | Self::DropUnique { .. }
            // A check enforces the shape of every new row; removing it is the same kind of
            // loss as removing a uniqueness guarantee, so it needs the same explicit flag.
            | Self::DropCheck { .. }
            // Rows that still hold an enum value no longer in the type are unreadable.
            | Self::DropEnum { .. }
            // A view that something else reads from stops resolving, and a sequence that
            // another default calls starts failing.
            | Self::DropView { .. }
            | Self::DropSequence { .. }
            | Self::RemoveEnumValue { .. } => Safety::Destructive,
            Self::DropForeignKey { .. } => Safety::Risky,
        }
    }

    /// The qualified name of the table this change applies to, if it applies to one.
    pub fn table(&self) -> Option<String> {
        match self {
            Self::CreateTable(table) => Some(table.qualified_name()),
            Self::DropTable { name } => Some(name.clone()),
            Self::CreateEnum(_)
            | Self::DropEnum { .. }
            | Self::AddEnumValue { .. }
            | Self::RemoveEnumValue { .. } => None,
            // A view or sequence is not a table: it has no columns to rebuild and SQLite's
            // table-rebuilding path must not treat it as one.
            Self::CreateView(_)
            | Self::ReplaceView(_)
            | Self::DropView { .. }
            | Self::CreateSequence(_)
            | Self::AlterSequence { .. }
            | Self::DropSequence { .. } => None,
            Self::AddColumn { table, .. }
            | Self::DropColumn { table, .. }
            | Self::AlterColumn { table, .. }
            | Self::RenameColumn { table, .. }
            | Self::AddIndex { table, .. }
            | Self::DropIndex { table, .. }
            | Self::AddUnique { table, .. }
            | Self::DropUnique { table, .. }
            | Self::AddForeignKey { table, .. }
            | Self::DropForeignKey { table, .. }
            | Self::AddCheck { table, .. }
            | Self::DropCheck { table, .. }
            | Self::EnableRls { table } => Some(table.clone()),
        }
    }
}

impl core::fmt::Display for Change {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CreateTable(t) => write!(f, "create table {}", t.qualified_name()),
            Self::DropTable { name } => write!(f, "drop table {name}"),
            Self::CreateEnum(e) => write!(f, "create enum {}", e.name),
            Self::DropEnum { name } => write!(f, "drop enum {name}"),
            Self::AddEnumValue { name, value } => write!(f, "add value {value} to enum {name}"),
            Self::RemoveEnumValue { name, value } => {
                write!(f, "remove value {value} from enum {name}")
            }
            Self::AddColumn { table, column } => {
                write!(f, "add column {table}.{}", column.name)
            }
            Self::DropColumn { table, column } => write!(f, "drop column {table}.{column}"),
            Self::AlterColumn { table, to, .. } => {
                write!(f, "alter column {table}.{}", to.name)
            }
            Self::RenameColumn { table, from, to } => {
                write!(f, "rename column {table}.{from} to {to}")
            }
            Self::AddIndex { table, index } => write!(f, "create index {} on {table}", index.name),
            Self::DropIndex { table, index } => write!(f, "drop index {} on {table}", index.name),
            Self::AddUnique { table, unique } => {
                write!(f, "add unique constraint {} on {table}", unique.name)
            }
            Self::DropUnique { table, unique } => {
                write!(f, "drop unique constraint {} on {table}", unique.name)
            }
            Self::AddForeignKey { table, foreign_key } => {
                write!(f, "add foreign key {} on {table}", foreign_key.name)
            }
            Self::DropForeignKey { table, foreign_key } => {
                write!(f, "drop foreign key {} on {table}", foreign_key.name)
            }
            Self::AddCheck { table, check } => write!(f, "add check {} on {table}", check.name),
            Self::DropCheck { table, check } => write!(f, "drop check {} on {table}", check.name),
            Self::CreateView(view) => write!(f, "create view {}", view.name),
            Self::ReplaceView(view) => write!(f, "replace view {}", view.name),
            Self::DropView { name, .. } => write!(f, "drop view {name}"),
            Self::CreateSequence(sequence) => {
                write!(f, "create sequence {}", sequence.qualified_name())
            }
            Self::AlterSequence { name, .. } => write!(f, "alter sequence {name}"),
            Self::DropSequence { name } => write!(f, "drop sequence {name}"),
            Self::EnableRls { table } => write!(f, "enable row-level security on {table}"),
        }
    }
}

/// Compute the changes needed to turn `current` into `desired`.
///
/// The returned list is already in a safe application order: sequences and enums are created
/// before the tables that use them, tables are created parents-first and views after the
/// tables they read; on the way out, views are dropped before their tables and sequences
/// after them. Nothing is ever renamed automatically — a rename looks exactly like a drop
/// plus an add, so Besu refuses to guess. Declare the rename explicitly with a migration if
/// you want to keep the data.
pub fn diff(current: &Schema, desired: &Schema) -> Vec<Change> {
    diff_with(current, desired, &|ty| ty.clone())
}

/// Compute the changes needed to turn `current` into `desired`, folding types that the
/// dialect cannot tell apart.
///
/// SQLite has a single integer type, so a schema declaring `i32` and a database reporting
/// `INTEGER` must compare equal or `push` would never converge. Each dialect supplies its
/// own normalisation through [`Ddl::normalise_type`](crate::Ddl::normalise_type).
pub fn diff_with(
    current: &Schema,
    desired: &Schema,
    normalise: &dyn Fn(&besu_core::TypeDef) -> besu_core::TypeDef,
) -> Vec<Change> {
    let mut changes = Vec::new();

    // Only the schemas the declared schema describes are managed. A database can hold
    // tables that belong to something else — another application, an extension, a schema
    // somebody made by hand — and reporting those as drops would make `push` destructive
    // over a schema it was never told about. The default schema is always managed, because
    // an unqualified table lives there.
    let mut managed: Vec<Option<&str>> = vec![None];
    for table in &desired.tables {
        if let Some(schema) = table.schema.as_deref()
            && !managed.contains(&Some(schema))
        {
            managed.push(Some(schema));
        }
    }
    // A view and a sequence can name a schema no table does, and the declaration is then
    // the thing that says the schema belongs to this application.
    for view in &desired.views {
        if let Some(schema) = view.schema()
            && !managed.contains(&Some(schema))
        {
            managed.push(Some(schema));
        }
    }
    for sequence in &desired.sequences {
        if let Some(schema) = sequence.schema.as_deref()
            && !managed.contains(&Some(schema))
        {
            managed.push(Some(schema));
        }
    }

    // No `TableDef` can describe an enum type, so a schema built from table definitions
    // carries no enum information at all. A type is therefore only dropped when a column
    // currently uses it and nothing the declared schema names still does: that is the one
    // case where Besu can be sure the type belongs to this schema rather than to something
    // else in the same database.
    fn required_by_a_column(schema: &Schema, name: &str, managed: &[Option<&str>]) -> bool {
        schema
            .tables
            .iter()
            .filter(|table| managed.contains(&table.schema.as_deref()))
            .any(|table| {
                table.columns.iter().any(|column| {
                    matches!(&column.ty, besu_core::TypeDef::Custom(ty) if ty.as_ref() == name)
                })
            })
    }

    // Enums first: a table's column can name an enum type, so the type has to exist before
    // the table that uses it is created.
    for desired_enum in &desired.enums {
        if current.find_enum(&desired_enum.name).is_none() {
            changes.push(Change::CreateEnum(desired_enum.clone()));
        }
    }
    for desired_enum in &desired.enums {
        let Some(existing) = current.find_enum(&desired_enum.name) else {
            continue;
        };
        for value in &desired_enum.values {
            if !existing.values.contains(value) {
                changes.push(Change::AddEnumValue {
                    name: desired_enum.name.clone(),
                    value: value.clone(),
                });
            }
        }
        for value in &existing.values {
            if !desired_enum.values.contains(value) {
                changes.push(Change::RemoveEnumValue {
                    name: desired_enum.name.clone(),
                    value: value.clone(),
                });
            }
        }
    }

    // Create tables that are missing, in dependency order.
    for table in desired.topological_order() {
        if current.find(table.schema.as_deref(), &table.name).is_none() {
            changes.push(Change::CreateTable(table.clone()));
        }
    }

    // Sequences: created before the tables that might default to them, altered in place,
    // and dropped after the tables that use them.
    for sequence in &desired.sequences {
        match current.find_sequence(sequence.schema.as_deref(), &sequence.name) {
            None => changes.push(Change::CreateSequence(sequence.clone())),
            Some(existing) if existing.normalised() != sequence.normalised() => {
                changes.push(Change::AlterSequence {
                    name: sequence.qualified_name(),
                    from: existing.clone(),
                    to: sequence.clone(),
                });
            }
            Some(_) => {}
        }
    }

    // Views: a view over a table created in this same run is safe because every
    // `CreateView` sorts after every `CreateTable`. The order among the views themselves
    // follows their dependencies.
    for view in desired.ordered_views() {
        match current.view(&view.name) {
            None => changes.push(Change::CreateView(view.clone())),
            Some(existing)
                if existing.materialised != view.materialised
                    || crate::normalise_view_definition(&existing.definition)
                        != crate::normalise_view_definition(&view.definition) =>
            {
                changes.push(Change::ReplaceView(view.clone()));
            }
            Some(_) => {}
        }
    }

    // Modify tables that exist on both sides.
    for table in &desired.tables {
        let Some(existing) = current.find(table.schema.as_deref(), &table.name) else {
            continue;
        };
        diff_table(existing, table, normalise, &mut changes);
    }

    // Row-level security can be turned on but never off: see [`Change::EnableRls`].
    for table in &desired.tables {
        let Some(existing) = current.find(table.schema.as_deref(), &table.name) else {
            continue;
        };
        if table.rls && !existing.rls {
            changes.push(Change::EnableRls {
                table: table.qualified_name(),
            });
        }
    }

    // Drop views that are no longer wanted, before the tables they might read from.
    for view in current.ordered_views().into_iter().rev() {
        let managed_here = match view.schema() {
            Some(schema) => managed.contains(&Some(schema)),
            None => managed.contains(&None),
        };
        if !managed_here {
            continue;
        }
        if desired.view(&view.name).is_none() {
            changes.push(Change::DropView {
                name: view.name.clone(),
                materialised: view.materialised,
            });
        }
    }

    // Drop tables that are no longer wanted, children first.
    let ordered = current.topological_order();
    for table in ordered.into_iter().rev() {
        if !managed.contains(&table.schema.as_deref()) {
            continue;
        }
        if desired.find(table.schema.as_deref(), &table.name).is_none() {
            changes.push(Change::DropTable {
                name: table.qualified_name(),
            });
        }
    }

    // Sequences last: a table that defaults to one is already gone by this point.
    for sequence in &current.sequences {
        if !managed.contains(&sequence.schema.as_deref()) {
            continue;
        }
        if desired
            .find_sequence(sequence.schema.as_deref(), &sequence.name)
            .is_none()
        {
            changes.push(Change::DropSequence {
                name: sequence.qualified_name(),
            });
        }
    }

    // Enums last: the tables that use them are already gone by this point.
    for existing in &current.enums {
        if desired.find_enum(&existing.name).is_some() {
            continue;
        }
        // A type a declared column still names cannot be dropped yet: the column has to
        // change first, and the next push drops the type.
        if required_by_a_column(desired, &existing.name, &managed) {
            continue;
        }
        // Only drop a type that a table in the current schema is using. Anything else may
        // belong to an application `push` was never told about.
        if !required_by_a_column(current, &existing.name, &managed) {
            continue;
        }
        changes.push(Change::DropEnum {
            name: existing.name.clone(),
        });
    }

    // Order within a table. Tables and types come first, then per-table changes, then the
    // destructive tail. A replaced object (an index or constraint whose definition changed)
    // must have its `Drop` run before its `Add`, because both use the same name and the
    // database rejects a second object with it, so the drops of those objects are keyed
    // before their adds.
    changes.sort_by_key(|change| match change {
        // A view that is going away must go before its columns change or disappear, and
        // before a table rebuild drops and recreates them.
        Change::DropView { .. } => 0,
        // Sequences next: a column default may call one.
        Change::CreateSequence(_) => 1,
        Change::CreateEnum(_) => 2,
        Change::CreateTable(_) => 3,
        Change::AddColumn { .. } => 4,
        Change::RenameColumn { .. } => 5,
        Change::AlterColumn { .. } => 6,
        Change::AddEnumValue { .. } => 7,
        Change::AlterSequence { .. } => 8,
        Change::DropForeignKey { .. } => 9,
        // Dropping a check before the column it names avoids "constraint does not exist":
        // the database drops a constraint of its own when its last column goes.
        Change::DropCheck { .. } => 10,
        Change::DropUnique { .. } => 11,
        Change::DropIndex { .. } => 12,
        Change::AddIndex { .. } => 13,
        Change::AddUnique { .. } => 14,
        Change::AddForeignKey { .. } => 15,
        // A check may name a column added above, so it comes after every additive change.
        Change::AddCheck { .. } => 16,
        // A replaced view may name a column added above, and must stop naming one that is
        // about to be dropped, so it sits between the additive and destructive tails.
        Change::ReplaceView(_) => 17,
        // A new view needs every table it reads, and their indexes, to exist first. The
        // relative order among the views comes from `ordered_views` and is preserved by the
        // stable sort.
        Change::CreateView(_) => 18,
        Change::EnableRls { .. } => 19,
        Change::DropColumn { .. } => 20,
        Change::DropTable { .. } => 21,
        Change::RemoveEnumValue { .. } => 22,
        Change::DropEnum { .. } => 23,
        // A table that defaults to a sequence has to be gone before the sequence is.
        Change::DropSequence { .. } => 24,
    });

    changes
}

/// The name a column used to have, if that name still exists on the live table.
fn renamed_previous<'a>(column: &'a Column, current: &Table) -> Option<&'a str> {
    let previous = column.renamed_from.as_deref()?;
    if current.column(previous).is_some() && current.column(&column.name).is_none() {
        Some(previous)
    } else {
        None
    }
}

fn diff_table(
    current: &Table,
    desired: &Table,
    normalise: &dyn Fn(&besu_core::TypeDef) -> besu_core::TypeDef,
    changes: &mut Vec<Change>,
) {
    let table = desired.qualified_name();

    // Renames first: a declared `renamed_from` turns what would be a drop plus an add into
    // one statement that keeps the data.
    for column in &desired.columns {
        if current.column(&column.name).is_some() {
            continue;
        }
        if let Some(previous) = &column.renamed_from
            && current.column(previous).is_some()
        {
            changes.push(Change::RenameColumn {
                table: table.clone(),
                from: previous.clone(),
                to: column.name.clone(),
            });
        }
    }

    // Added columns.
    for column in &desired.columns {
        if current.column(&column.name).is_none() && renamed_previous(column, current).is_none() {
            changes.push(Change::AddColumn {
                table: table.clone(),
                column: column.clone(),
            });
        }
    }

    // Removed columns.
    // A generated column is dropped before the column its expression reads: on SQLite a
    // drop is a plain `ALTER TABLE ... DROP COLUMN`, and the server refuses to remove a
    // column that a generated expression still references.
    let mut removed: Vec<&Column> = current
        .columns
        .iter()
        .filter(|column| {
            let renamed_away = desired.columns.iter().any(|desired| {
                desired.renamed_from.as_deref() == Some(column.name.as_str())
                    && current.column(&desired.name).is_none()
            });
            desired.column(&column.name).is_none() && !renamed_away
        })
        .collect();
    removed.sort_by_key(|column| column.generated.is_none());

    for column in removed {
        changes.push(Change::DropColumn {
            table: table.clone(),
            column: column.name.clone(),
        });
    }

    // Changed columns.
    for column in &desired.columns {
        let mut renamed_existing;
        let existing = match current.column(&column.name) {
            Some(existing) => existing,
            // After a rename the definition lives under the old name, so compare against
            // that — with the column's new name in place, because that is what the table
            // will have by the time the alter runs.
            None => match renamed_previous(column, current)
                .and_then(|previous| current.column(previous))
            {
                Some(previous) => {
                    renamed_existing = previous.clone();
                    renamed_existing.name = column.name.clone();
                    &renamed_existing
                }
                None => continue,
            },
        };
        if !existing.same_shape(column, normalise) {
            changes.push(Change::AlterColumn {
                table: table.clone(),
                from: existing.clone(),
                to: column.clone(),
            });
        }
    }

    // Indexes.
    for index in &desired.indexes {
        match current.indexes.iter().find(|i| i.name == index.name) {
            Some(existing) if existing == index => {}
            Some(_) => {
                changes.push(Change::DropIndex {
                    table: table.clone(),
                    index: index.clone(),
                });
                changes.push(Change::AddIndex {
                    table: table.clone(),
                    index: index.clone(),
                });
            }
            None => changes.push(Change::AddIndex {
                table: table.clone(),
                index: index.clone(),
            }),
        }
    }
    for index in &current.indexes {
        if !desired.indexes.iter().any(|i| i.name == index.name) {
            changes.push(Change::DropIndex {
                table: table.clone(),
                index: index.clone(),
            });
        }
    }

    // Multi-column `UNIQUE` constraints. They are matched by the columns they cover rather
    // than by name: SQLite does not report the name of a `UNIQUE` constraint, so the name
    // it comes back with is an internal one.
    for unique in &desired.uniques {
        if !current
            .uniques
            .iter()
            .any(|existing| existing.columns == unique.columns)
        {
            changes.push(Change::AddUnique {
                table: table.clone(),
                unique: unique.clone(),
            });
        }
    }
    for unique in &current.uniques {
        if !desired
            .uniques
            .iter()
            .any(|wanted| wanted.columns == unique.columns)
        {
            changes.push(Change::DropUnique {
                table: table.clone(),
                unique: unique.clone(),
            });
        }
    }

    // Foreign keys.
    for fk in &desired.foreign_keys {
        match current.foreign_keys.iter().find(|f| f.name == fk.name) {
            Some(existing) if existing == fk => {}
            Some(_) => {
                changes.push(Change::DropForeignKey {
                    table: table.clone(),
                    foreign_key: fk.clone(),
                });
                changes.push(Change::AddForeignKey {
                    table: table.clone(),
                    foreign_key: fk.clone(),
                });
            }
            None => changes.push(Change::AddForeignKey {
                table: table.clone(),
                foreign_key: fk.clone(),
            }),
        }
    }
    for fk in &current.foreign_keys {
        if !desired.foreign_keys.iter().any(|f| f.name == fk.name) {
            changes.push(Change::DropForeignKey {
                table: table.clone(),
                foreign_key: fk.clone(),
            });
        }
    }

    // `CHECK` constraints. Matched by name, because that is how the statement that drops one
    // finds it, and compared after normalising parentheses: PostgreSQL reports
    // `CHECK (value > 0)` as `(value > 0)`, which must equal the declared `value > 0`.
    fn same_check(a: &Check, b: &Check) -> bool {
        crate::normalise_expression(&a.expression) == crate::normalise_expression(&b.expression)
    }
    for check in &desired.checks {
        match current.checks.iter().find(|c| c.name == check.name) {
            Some(existing) if same_check(existing, check) => {}
            Some(_) => {
                changes.push(Change::DropCheck {
                    table: table.clone(),
                    check: check.clone(),
                });
                changes.push(Change::AddCheck {
                    table: table.clone(),
                    check: check.clone(),
                });
            }
            None => changes.push(Change::AddCheck {
                table: table.clone(),
                check: check.clone(),
            }),
        }
    }
    for check in &current.checks {
        if !desired.checks.iter().any(|c| c.name == check.name) {
            changes.push(Change::DropCheck {
                table: table.clone(),
                check: check.clone(),
            });
        }
    }
}

/// The most dangerous change in a list, if any.
pub fn worst_safety(changes: &[Change]) -> Safety {
    changes
        .iter()
        .map(Change::safety)
        .max()
        .unwrap_or(Safety::Safe)
}

#[cfg(test)]
mod tests {
    use super::{diff, worst_safety};
    use crate::{Change, Column, Enum, Safety, Schema, Table, Unique};
    use besu_core::TypeDef;

    fn column(name: &str, ty: TypeDef) -> Column {
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
        }
    }

    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table {
            name: name.into(),
            schema: None,
            columns,
            indexes: Vec::new(),
            uniques: Vec::new(),
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            comment: None,
            rls: false,
        }
    }

    fn schema(tables: Vec<Table>) -> Schema {
        Schema {
            tables,
            enums: Vec::new(),
            views: Vec::new(),
            ..Schema::default()
        }
    }

    #[test]
    fn creating_from_nothing_only_creates_tables() {
        let desired = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        let changes = diff(&Schema::new(), &desired);
        assert_eq!(changes.len(), 1);
        assert_eq!(worst_safety(&changes), Safety::Safe);
    }

    #[test]
    fn identical_schemas_produce_no_changes() {
        let a = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        let b = a.clone();
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn adding_a_nullable_column_is_safe() {
        let before = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        let mut nullable = column("bio", TypeDef::Text);
        nullable.nullable = true;
        let after = schema(vec![table(
            "users",
            vec![column("id", TypeDef::Int8), nullable],
        )]);
        let changes = diff(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(worst_safety(&changes), Safety::Safe);
    }

    #[test]
    fn dropping_a_column_is_destructive() {
        let before = schema(vec![table(
            "users",
            vec![column("id", TypeDef::Int8), column("bio", TypeDef::Text)],
        )]);
        let after = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        let changes = diff(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].safety(), Safety::Destructive);
    }

    #[test]
    fn changing_a_column_type_is_risky() {
        let before = schema(vec![table("users", vec![column("age", TypeDef::Int4)])]);
        let after = schema(vec![table("users", vec![column("age", TypeDef::Int8)])]);
        let changes = diff(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].safety(), Safety::Risky);
    }

    fn mood(values: &[&str]) -> Enum {
        Enum {
            name: "mood".into(),
            values: values.iter().map(|value| (*value).into()).collect(),
        }
    }

    #[test]
    fn an_enum_is_created_before_the_table_that_uses_it() {
        let mut reviews = table(
            "reviews",
            vec![column("mood", TypeDef::Custom("mood".into()))],
        );
        reviews.uniques = Vec::new();
        let desired = Schema {
            tables: vec![reviews],
            enums: vec![mood(&["sad", "happy"])],
            views: Vec::new(),
            ..Schema::default()
        };

        let changes = diff(&Schema::new(), &desired);
        assert!(matches!(&changes[0], Change::CreateEnum(e) if e.name == "mood"));
        assert!(matches!(&changes[1], Change::CreateTable(t) if t.name == "reviews"));
    }

    #[test]
    fn appending_an_enum_value_is_risky() {
        let current = Schema {
            tables: Vec::new(),
            enums: vec![mood(&["sad", "happy"])],
            views: Vec::new(),
            ..Schema::default()
        };
        let desired = Schema {
            tables: Vec::new(),
            enums: vec![mood(&["sad", "happy", "ecstatic"])],
            views: Vec::new(),
            ..Schema::default()
        };

        let changes = diff(&current, &desired);
        assert_eq!(
            changes,
            [Change::AddEnumValue {
                name: "mood".into(),
                value: "ecstatic".into(),
            }]
        );
        assert_eq!(changes[0].safety(), Safety::Risky);
    }

    #[test]
    fn removing_an_enum_value_is_destructive() {
        let current = Schema {
            tables: Vec::new(),
            enums: vec![mood(&["sad", "happy"])],
            views: Vec::new(),
            ..Schema::default()
        };
        let desired = Schema {
            tables: Vec::new(),
            enums: vec![mood(&["sad"])],
            views: Vec::new(),
            ..Schema::default()
        };

        let changes = diff(&current, &desired);
        assert_eq!(
            changes,
            [Change::RemoveEnumValue {
                name: "mood".into(),
                value: "happy".into(),
            }]
        );
        assert_eq!(worst_safety(&changes), Safety::Destructive);
    }

    #[test]
    fn dropping_an_enum_that_a_table_used_is_destructive() {
        let current = Schema {
            tables: vec![table(
                "reviews",
                vec![column("mood", TypeDef::Custom("mood".into()))],
            )],
            enums: vec![mood(&["sad"])],
            views: Vec::new(),
            ..Schema::default()
        };

        let changes = diff(&current, &Schema::new());
        // The table goes first, then the type it was the last user of.
        assert!(matches!(&changes[0], Change::DropTable { name } if name == "reviews"));
        assert!(matches!(&changes[1], Change::DropEnum { name } if name == "mood"));
        assert_eq!(worst_safety(&changes), Safety::Destructive);
    }

    #[test]
    fn an_enum_nothing_uses_is_left_alone() {
        let current = Schema {
            tables: Vec::new(),
            enums: vec![mood(&["sad"])],
            views: Vec::new(),
            ..Schema::default()
        };
        assert!(diff(&current, &Schema::new()).is_empty());
    }

    #[test]
    fn a_composite_unique_is_added_and_dropped() {
        let before = table(
            "memberships",
            vec![
                column("user_id", TypeDef::Int8),
                column("team_id", TypeDef::Int8),
            ],
        );
        let mut after = before.clone();
        after.uniques.push(Unique {
            name: "memberships_user_team_key".into(),
            columns: vec!["user_id".into(), "team_id".into()],
        });

        let added = diff(&schema(vec![before.clone()]), &schema(vec![after.clone()]));
        assert_eq!(added.len(), 1);
        assert!(matches!(&added[0], Change::AddUnique { table, .. } if table == "memberships"));
        assert_eq!(added[0].safety(), Safety::Risky);

        let dropped = diff(&schema(vec![after]), &schema(vec![before]));
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].safety(), Safety::Destructive);
    }

    #[test]
    fn a_unique_is_matched_by_its_columns() {
        // SQLite reports a `UNIQUE` constraint under an internal name, so a name that
        // differs must not be reported as a change.
        let mut current = table(
            "memberships",
            vec![
                column("user_id", TypeDef::Int8),
                column("team_id", TypeDef::Int8),
            ],
        );
        current.uniques.push(Unique {
            name: "sqlite_autoindex_memberships_1".into(),
            columns: vec!["user_id".into(), "team_id".into()],
        });

        let mut desired = current.clone();
        desired.uniques[0].name = "memberships_user_team_key".into();

        assert!(diff(&schema(vec![current]), &schema(vec![desired])).is_empty());
    }

    #[test]
    fn the_same_name_in_two_schemas_is_two_tables() {
        let mut scoped = table("things", vec![column("id", TypeDef::Int8)]);
        scoped.schema = Some("app".into());
        let desired = schema(vec![
            table("things", vec![column("id", TypeDef::Int8)]),
            scoped,
        ]);

        let changes = diff(&Schema::new(), &desired);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|change| matches!(
            change,
            Change::CreateTable(table) if table.qualified_name() == "app.things"
        )));

        // With both in the database, neither is re-created.
        assert!(diff(&desired, &desired).is_empty());
    }

    #[test]
    fn a_table_in_a_schema_the_declared_schema_never_names_is_left_alone() {
        let mut scoped = table("things", vec![column("id", TypeDef::Int8)]);
        scoped.schema = Some("somebody_else".into());
        let current = schema(vec![
            table("things", vec![column("id", TypeDef::Int8)]),
            scoped,
        ]);

        let desired = schema(vec![table("things", vec![column("id", TypeDef::Int8)])]);
        assert!(diff(&current, &desired).is_empty());
    }

    fn check(name: &str, expression: &str) -> crate::Check {
        crate::Check {
            name: name.into(),
            expression: expression.into(),
        }
    }

    #[test]
    fn a_check_is_added_risky_and_dropped_destructively() {
        let before = table("accounts", vec![column("balance", TypeDef::Int8)]);
        let mut after = before.clone();
        after
            .checks
            .push(check("accounts_balance_check", "balance >= 0"));

        let added = diff(&schema(vec![before.clone()]), &schema(vec![after.clone()]));
        assert_eq!(added.len(), 1);
        assert_eq!(
            added[0],
            Change::AddCheck {
                table: "accounts".into(),
                check: check("accounts_balance_check", "balance >= 0"),
            }
        );
        assert_eq!(added[0].safety(), Safety::Risky);

        let dropped = diff(&schema(vec![after]), &schema(vec![before]));
        assert_eq!(
            dropped,
            [Change::DropCheck {
                table: "accounts".into(),
                check: check("accounts_balance_check", "balance >= 0"),
            }]
        );
        assert_eq!(dropped[0].safety(), Safety::Destructive);
    }

    #[test]
    fn a_check_converges_despite_extra_parentheses() {
        // PostgreSQL reports `CHECK (balance >= 0)` as `(balance >= 0)`.
        let mut current = table("accounts", vec![column("balance", TypeDef::Int8)]);
        current
            .checks
            .push(check("accounts_balance_check", "(balance >= 0)"));

        let mut desired = current.clone();
        desired.checks[0].expression = "balance >= 0".into();

        assert!(diff(&schema(vec![current]), &schema(vec![desired])).is_empty());
    }

    #[test]
    fn a_changed_check_is_replaced() {
        let mut before = table("accounts", vec![column("balance", TypeDef::Int8)]);
        before
            .checks
            .push(check("accounts_balance_check", "balance >= 0"));
        let mut after = before.clone();
        after.checks[0].expression = "balance > 0".into();

        let changes = diff(&schema(vec![before]), &schema(vec![after]));
        // The drop has to run first: both statements use the same constraint name.
        assert!(matches!(&changes[0], Change::DropCheck { .. }));
        assert!(matches!(&changes[1], Change::AddCheck { .. }));
        assert_eq!(worst_safety(&changes), Safety::Destructive);
    }

    #[test]
    fn an_index_predicate_is_part_of_the_index() {
        let mut before = table("events", vec![column("kind", TypeDef::Text)]);
        before.indexes.push(crate::Index {
            name: "events_kind_idx".into(),
            columns: vec!["kind".into()],
            unique: false,
            kind: crate::IndexKind::Btree,
            predicate: None,
        });
        let mut after = before.clone();
        after.indexes[0].predicate = Some("kind = 'error'".into());

        // The name is the same, but the index covers different rows, so it is recreated.
        let changes = diff(&schema(vec![before.clone()]), &schema(vec![after.clone()]));
        assert!(matches!(&changes[0], Change::DropIndex { .. }));
        assert!(matches!(&changes[1], Change::AddIndex { .. }));

        assert!(diff(&schema(vec![after.clone()]), &schema(vec![after])).is_empty());
    }

    #[test]
    fn a_view_is_created_after_its_table_and_dropped_before_it() {
        let mut desired = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        desired.add_view(crate::View::new(
            "active_users",
            "SELECT id FROM users WHERE active",
        ));

        let changes = diff(&Schema::new(), &desired);
        assert!(matches!(&changes[0], Change::CreateTable(table) if table.name == "users"));
        assert!(matches!(&changes[1], Change::CreateView(view) if view.name == "active_users"));
        assert_eq!(worst_safety(&changes), Safety::Safe);

        // A view is dropped before the table it reads.
        let dropped = diff(&desired, &Schema::new());
        assert!(matches!(&dropped[0], Change::DropView { name, .. } if name == "active_users"));
        assert!(matches!(&dropped[1], Change::DropTable { name } if name == "users"));
        assert_eq!(worst_safety(&dropped), Safety::Destructive);

        // And an identical view is nothing.
        assert!(diff(&desired, &desired).is_empty());
    }

    #[test]
    fn a_changed_view_is_replaced_risky() {
        let mut before = schema(Vec::new());
        before.add_view(crate::View::new("active_users", "SELECT id FROM users"));
        let mut after = before.clone();
        after.views[0].definition = "SELECT id FROM users WHERE active".into();

        let changes = diff(&before, &after);
        assert_eq!(
            changes,
            [Change::ReplaceView(crate::View::new(
                "active_users",
                "SELECT id FROM users WHERE active"
            ))]
        );
        assert_eq!(changes[0].safety(), Safety::Risky);
    }

    #[test]
    fn a_view_definition_the_database_rewrites_still_converges() {
        // PostgreSQL qualifies every column; MySQL backticks every identifier; SQLite
        // repeats the `CREATE VIEW` header. None of that is a change.
        let mut current = schema(Vec::new());
        current.add_view(crate::View::new(
            "active_users",
            " SELECT users.id,\n    users.name\n   FROM users\n  WHERE users.active;",
        ));
        let mut desired = schema(Vec::new());
        desired.add_view(crate::View::new(
            "active_users",
            "SELECT id, name FROM users WHERE active",
        ));

        assert!(diff(&current, &desired).is_empty());
        assert!(diff(&desired, &current).is_empty());
    }

    #[test]
    fn a_view_over_a_view_is_created_in_dependency_order() {
        let mut desired = schema(Vec::new());
        desired.add_view(crate::View::new("a_first", "SELECT id FROM users"));
        desired.add_view(crate::View::new(
            "b_second",
            "SELECT id FROM a_first WHERE id > 0",
        ));

        let changes = diff(&Schema::new(), &desired);
        assert!(matches!(&changes[0], Change::CreateView(view) if view.name == "a_first"));
        assert!(matches!(&changes[1], Change::CreateView(view) if view.name == "b_second"));
    }

    #[test]
    fn a_view_in_a_schema_the_declared_schema_never_names_is_left_alone() {
        let mut current = schema(Vec::new());
        current.add_view(crate::View::new("somebody_else.their_view", "SELECT 1"));
        assert!(diff(&current, &schema(Vec::new())).is_empty());
    }

    fn sequence(name: &str) -> crate::Sequence {
        crate::Sequence::new(name)
    }

    #[test]
    fn a_sequence_is_created_and_dropped_in_the_right_order() {
        let mut desired = schema(vec![table("orders", vec![column("id", TypeDef::Int8)])]);
        let mut numbers = sequence("order_number_seq");
        numbers.start = 1000;
        desired.add_sequence(numbers);

        let created = diff(&Schema::new(), &desired);
        assert!(matches!(&created[0], Change::CreateSequence(s) if s.name == "order_number_seq"));
        assert_eq!(created[0].safety(), Safety::Safe);

        let dropped = diff(&desired, &Schema::new());
        assert!(matches!(&dropped[0], Change::DropTable { .. }));
        assert!(matches!(&dropped[1], Change::DropSequence { .. }));
        assert_eq!(worst_safety(&dropped), Safety::Destructive);
    }

    #[test]
    fn a_changed_sequence_is_alterable_and_a_default_bound_is_not_a_change() {
        let mut current = schema(Vec::new());
        current.add_sequence(sequence("order_number_seq"));
        let mut desired = current.clone();
        desired.sequences[0].start = 1000;

        let changes = diff(&current, &desired);
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0], Change::AlterSequence { .. }));
        assert_eq!(changes[0].safety(), Safety::Risky);

        // PostgreSQL reports the default bound as a number; a declaration that leaves it
        // out is the same sequence.
        let mut concrete = current.clone();
        concrete.sequences[0].min = Some(1);
        concrete.sequences[0].max = Some(i64::MAX);
        assert!(diff(&current, &concrete).is_empty());
        assert!(diff(&concrete, &current).is_empty());
    }

    #[test]
    fn an_index_kind_is_part_of_the_index() {
        let mut before = schema(vec![table(
            "events",
            vec![column("payload", TypeDef::Jsonb)],
        )]);
        before.tables[0].indexes.push(crate::Index {
            name: "events_payload_idx".into(),
            columns: vec!["payload".into()],
            unique: false,
            kind: crate::IndexKind::Btree,
            predicate: None,
        });
        let mut after = before.clone();
        after.tables[0].indexes[0].kind = crate::IndexKind::Gin;

        let changes = diff(&before, &after);
        // Both statements use the same name, so the drop has to run first.
        assert!(matches!(&changes[0], Change::DropIndex { .. }));
        assert!(matches!(&changes[1], Change::AddIndex { .. }));
        assert!(diff(&after, &after).is_empty());
    }

    #[test]
    fn a_generated_column_change_is_risky_and_adding_one_is_safe() {
        let before = schema(vec![table("users", vec![column("id", TypeDef::Int8)])]);
        let mut after = before.clone();
        after.tables[0].columns.push(crate::Column {
            name: "search".into(),
            ty: TypeDef::Text,
            nullable: true,
            auto_increment: false,
            primary_key: false,
            unique: false,
            default: None,
            generated: Some(crate::Generated {
                expression: "lower(id)".into(),
                stored: true,
            }),
            comment: None,
            renamed_from: None,
        });

        let added = diff(&before, &after);
        assert_eq!(added.len(), 1);
        assert!(matches!(&added[0], Change::AddColumn { .. }));
        // A generated column has no value of its own to invent, so it cannot fail the way a
        // `NOT NULL` column without a default does.
        assert_eq!(added[0].safety(), Safety::Safe);

        let mut changed = after.clone();
        if let Some(generated) = &mut changed.tables[0].columns[1].generated {
            generated.expression = "upper(id)".into();
        }
        let altered = diff(&after, &changed);
        assert_eq!(altered.len(), 1);
        assert!(matches!(&altered[0], Change::AlterColumn { .. }));
        assert_eq!(altered[0].safety(), Safety::Risky);
    }

    #[test]
    fn rls_is_enabled_but_never_disabled() {
        let mut desired = schema(vec![table("invoices", vec![column("id", TypeDef::Int8)])]);
        desired.tables[0].rls = true;

        let mut without_rls = desired.tables.clone();
        without_rls[0].rls = false;
        let enabled = diff(&schema(without_rls), &desired);
        assert_eq!(
            enabled,
            [Change::EnableRls {
                table: "invoices".into(),
            }]
        );
        assert_eq!(enabled[0].safety(), Safety::Safe);

        // The reverse is not a change: disabling RLS is a decision for a migration.
        let mut without_rls = desired.tables.clone();
        without_rls[0].rls = false;
        assert!(diff(&desired, &schema(without_rls)).is_empty());
    }

    #[test]
    fn policies_are_recorded_but_never_diffed() {
        let mut current = schema(vec![table("invoices", vec![column("id", TypeDef::Int8)])]);
        current.policies.push(crate::Policy {
            table: "invoices".into(),
            name: "own_invoices".into(),
            command: crate::PolicyCommand::All,
            roles: Vec::new(),
            using: Some("owner_id = current_setting('app.user_id')::uuid".into()),
            with_check: None,
        });

        // Removing the policy from the declaration does not drop it, and adding one does not
        // create it: the differ ignores the list in both directions.
        assert!(diff(&current, &schema(current.tables.clone())).is_empty());
        assert!(diff(&schema(current.tables.clone()), &current).is_empty());
    }
}
