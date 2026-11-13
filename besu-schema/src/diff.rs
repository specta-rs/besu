use crate::{
    Check, Column, Domain, Enum, ForeignKey, Index, Schema, Sequence, Table, Unique, View,
};

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
    /// A domain exists only in the desired schema.
    CreateDomain(Domain),
    /// A domain exists only in the current schema and nothing declares it.
    DropDomain {
        /// The domain's name.
        name: String,
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
}

impl Change {
    /// How dangerous this change is.
    pub fn safety(&self) -> Safety {
        match self {
            Self::CreateTable(_) | Self::CreateEnum(_) | Self::CreateDomain(_) | Self::CreateSequence(_) => {
                Safety::Safe
            }
            // A view is only a stored query, so creating one cannot touch a row.
            Self::CreateView(_) => Safety::Safe,
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
            // A domain's columns stop resolving when the type goes.
            | Self::DropDomain { .. }
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
            | Self::RemoveEnumValue { .. }
            | Self::CreateDomain(_)
            | Self::DropDomain { .. } => None,
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
            | Self::DropCheck { table, .. } => Some(table.clone()),
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
            Self::CreateDomain(domain) => write!(f, "create domain {}", domain.name),
            Self::DropDomain { name } => write!(f, "drop domain {name}"),
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

    // An enum type is only dropped when a column currently uses it and nothing the declared
    // schema names still does: that is the one case where Besu can be sure the type belongs
    // to this schema rather than to something else in the same database.
    fn required_by_a_column(schema: &Schema, name: &str, managed: &[Option<&str>]) -> bool {
        schema
            .tables
            .iter()
            .filter(|table| managed.contains(&table.schema.as_deref()))
            .any(|table| {
                table
                    .columns
                    .iter()
                    .any(|column| crate::ir::type_names(&column.ty, name))
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

    // Domains come after the enums they can be built on and before the tables whose columns
    // name them. One domain can be built on another, so they are added in dependency order.
    for desired_domain in desired.ordered_domains() {
        if current.find_domain(&desired_domain.name).is_none() {
            changes.push(Change::CreateDomain(desired_domain.clone()));
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

    // Domains: a table that uses one is already gone by this point, and an enum a domain is
    // built on must still exist, so this runs before the enum drop below.
    // A domain built on another has to go first, so the dependent is dropped first.
    for existing in current.ordered_domains().into_iter().rev() {
        if desired.find_domain(&existing.name).is_some() {
            continue;
        }
        if required_by_a_column(desired, &existing.name, &managed) {
            continue;
        }
        if !required_by_a_column(current, &existing.name, &managed) {
            continue;
        }
        changes.push(Change::DropDomain {
            name: existing.name.clone(),
        });
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
        // A domain can be built on an enum, so it follows the enum creation.
        Change::CreateDomain(_) => 2,
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
        Change::DropColumn { .. } => 20,
        Change::DropTable { .. } => 21,
        // A domain's columns are gone with the table, and an enum it is built on has to
        // outlive it.
        Change::DropDomain { .. } => 22,
        Change::RemoveEnumValue { .. } => 23,
        Change::DropEnum { .. } => 24,
        // A table that defaults to a sequence has to be gone before the sequence is.
        Change::DropSequence { .. } => 25,
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

/// Whether two indexes describe the same object.
///
/// The name is matched by the caller, because that is how a drop finds an index. Everything
/// else is part of the definition: the ordered key entries, each operator class, uniqueness,
/// the access method and a partial index's predicate. A change to any of them is a drop and
/// a create, since both statements use the same name.
fn same_index(a: &Index, b: &Index) -> bool {
    a.unique == b.unique
        && a.kind == b.kind
        && same_predicate(a.predicate.as_deref(), b.predicate.as_deref())
        && a.columns.len() == b.columns.len()
        && a.columns
            .iter()
            .zip(&b.columns)
            .all(|(a, b)| same_index_column(a, b))
}

/// Whether two partial-index predicates describe the same condition.
///
/// PostgreSQL reports the predicate with its own parentheses, quoting and keyword case, so it
/// is compared through the same expression normaliser a check uses.
fn same_predicate(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => crate::normalise_expression(a) == crate::normalise_expression(b),
        _ => false,
    }
}

/// Whether two key entries describe the same target and operator class.
///
/// Expressions are compared after folding identifier quoting and `::type` casts, because the
/// databases report them in their own spelling (`lower(\`email\`)` on MySQL, for example)
/// while the declaration is written as SQL. A column is compared exactly.
fn same_index_column(a: &crate::IndexColumn, b: &crate::IndexColumn) -> bool {
    if a.opclass != b.opclass {
        return false;
    }
    match (&a.target, &b.target) {
        (crate::IndexTarget::Column(a), crate::IndexTarget::Column(b)) => a == b,
        (crate::IndexTarget::Expression(a), crate::IndexTarget::Expression(b)) => {
            crate::normalise_generated_expression(a) == crate::normalise_generated_expression(b)
        }
        _ => false,
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
            Some(existing) if same_index(existing, index) => {}
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
    use crate::{Change, Column, Enum, IndexColumn, Safety, Schema, Table, Unique};
    use besu_core::{EnumDef, TypeDef};

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
    fn a_domain_is_created_before_the_table_that_uses_it() {
        let domain = besu_core::DomainDef::new("positive", &TypeDef::Int4);
        let mut priced = table(
            "priced",
            vec![column("amount", TypeDef::Domain(domain.clone()))],
        );
        priced.uniques = Vec::new();
        let desired = Schema {
            tables: vec![priced],
            domains: vec![crate::Domain {
                name: "positive".into(),
                base: TypeDef::Int4,
            }],
            ..Schema::default()
        };

        let changes = diff(&Schema::new(), &desired);
        assert!(
            matches!(&changes[0], Change::CreateDomain(domain) if domain.name == "positive"),
            "got {changes:?}"
        );
        assert!(matches!(&changes[1], Change::CreateTable(t) if t.name == "priced"));
    }

    #[test]
    fn dropping_a_domain_that_a_table_used_is_destructive_and_comes_after_the_table() {
        let domain = besu_core::DomainDef::new("positive", &TypeDef::Int4);
        let current = Schema {
            tables: vec![table(
                "priced",
                vec![column("amount", TypeDef::Domain(domain))],
            )],
            domains: vec![crate::Domain {
                name: "positive".into(),
                base: TypeDef::Int4,
            }],
            ..Schema::default()
        };

        let changes = diff(&current, &Schema::new());
        assert!(matches!(&changes[0], Change::DropTable { name } if name == "priced"));
        assert!(matches!(&changes[1], Change::DropDomain { name } if name == "positive"));
        assert_eq!(worst_safety(&changes), Safety::Destructive);
    }

    #[test]
    fn a_domain_nothing_uses_is_left_alone() {
        let current = Schema {
            domains: vec![crate::Domain {
                name: "positive".into(),
                base: TypeDef::Int4,
            }],
            ..Schema::default()
        };
        assert!(diff(&current, &Schema::new()).is_empty());
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

    fn enum_def(labels: &[&str]) -> TypeDef {
        TypeDef::Enum(EnumDef::owned("mood", labels.iter().copied()))
    }

    #[test]
    fn a_changed_inline_enum_label_set_is_an_alter_column() {
        // A MySQL enum is inline, so its labels are part of the column's type. Adding,
        // removing or reordering a label has to be a diff of that one column, never a silent
        // drop and re-add of the table.
        let before = schema(vec![table(
            "reviews",
            vec![column("mood", enum_def(&["happy", "sad"]))],
        )]);
        let after = schema(vec![table(
            "reviews",
            vec![column("mood", enum_def(&["happy", "meh", "sad"]))],
        )]);

        let changes = diff(&before, &after);
        assert_eq!(
            changes,
            [Change::AlterColumn {
                table: "reviews".into(),
                from: column("mood", enum_def(&["happy", "sad"])),
                to: column("mood", enum_def(&["happy", "meh", "sad"])),
            }]
        );
        assert_eq!(changes[0].safety(), Safety::Risky);

        // Reordering is a change too: PostgreSQL compares in declaration order.
        let reordered = schema(vec![table(
            "reviews",
            vec![column("mood", enum_def(&["sad", "happy"]))],
        )]);
        assert!(matches!(
            diff(&before, &reordered).as_slice(),
            [Change::AlterColumn { .. }]
        ));
    }

    #[test]
    fn a_static_and_an_owned_label_set_are_not_a_change() {
        // A declaration holds static labels and introspection produces owned ones; the two
        // have to compare equal or every push would alter the column.
        let before = schema(vec![table(
            "reviews",
            vec![column(
                "mood",
                TypeDef::Enum(EnumDef::new("mood", &["happy", "sad"])),
            )],
        )]);
        let after = schema(vec![table(
            "reviews",
            vec![column("mood", enum_def(&["happy", "sad"]))],
        )]);
        assert!(diff(&before, &after).is_empty());
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
    fn an_index_entry_change_is_a_drop_and_create() {
        let mut before = table(
            "users",
            vec![
                column("name", TypeDef::Text),
                column("email", TypeDef::Text),
            ],
        );
        before.indexes.push(crate::Index {
            name: "users_name_email_idx".into(),
            columns: vec![IndexColumn::column("name"), IndexColumn::column("email")],
            unique: false,
            kind: crate::IndexKind::Btree,
            predicate: None,
        });
        let mut after = before.clone();
        after.indexes[0].columns.reverse();

        let changes = diff(&schema(vec![before]), &schema(vec![after]));
        assert!(matches!(&changes[0], Change::DropIndex { .. }));
        assert!(matches!(&changes[1], Change::AddIndex { .. }));
    }

    #[test]
    fn an_operator_class_is_part_of_the_index() {
        let mut before = table("users", vec![column("email", TypeDef::Text)]);
        before.indexes.push(crate::Index {
            name: "users_email_idx".into(),
            columns: vec![IndexColumn::column("email")],
            unique: false,
            kind: crate::IndexKind::Btree,
            predicate: None,
        });
        let mut after = before.clone();
        after.indexes[0].columns[0].opclass = Some("text_pattern_ops".into());
        let (before, after) = (schema(vec![before]), schema(vec![after]));

        // `text_pattern_ops` changes which predicates the index serves, so the declaration
        // must replace the old index rather than keep it.
        let changes = diff(&before, &after);
        assert!(matches!(&changes[0], Change::DropIndex { .. }));
        assert!(matches!(&changes[1], Change::AddIndex { .. }));
        assert!(diff(&after, &after).is_empty());
    }

    #[test]
    fn an_expression_the_database_rewrites_still_converges() {
        // MySQL reports a functional key part as `lower(\`email\`)`, and PostgreSQL may add
        // its own quoting. Identifier quoting is not a change.
        let mut current = table("users", vec![column("email", TypeDef::Text)]);
        current.indexes.push(crate::Index {
            name: "users_lower_email_idx".into(),
            columns: vec![IndexColumn::expression("lower(`email`)")],
            unique: false,
            kind: crate::IndexKind::Btree,
            predicate: None,
        });
        let mut desired = current.clone();
        desired.indexes[0].columns[0] = IndexColumn::expression("lower(email)");

        assert!(diff(&schema(vec![current]), &schema(vec![desired])).is_empty());
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
}
