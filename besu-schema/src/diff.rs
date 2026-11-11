use crate::{Column, ForeignKey, Index, Schema, Table};

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
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// The table exists only in the desired schema.
    CreateTable(Table),
    /// The table exists only in the current schema.
    DropTable {
        /// The table being removed.
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
}

impl Change {
    /// How dangerous this change is.
    pub fn safety(&self) -> Safety {
        match self {
            Self::CreateTable(_) => Safety::Safe,
            Self::AddColumn { column, .. } => {
                if column.nullable || column.default.is_some() {
                    Safety::Safe
                } else {
                    // Adding a NOT NULL column without a default fails on a non-empty table.
                    Safety::Risky
                }
            }
            Self::AddIndex { .. } | Self::AddForeignKey { .. } => Safety::Risky,
            Self::AlterColumn { from, to, .. } => {
                if from.ty == to.ty
                    && from.nullable == to.nullable
                    && from.default == to.default
                    && from.auto_increment == to.auto_increment
                {
                    Safety::Safe
                } else {
                    Safety::Risky
                }
            }
            Self::DropTable { .. } | Self::DropColumn { .. } | Self::DropIndex { .. } => {
                Safety::Destructive
            }
            Self::DropForeignKey { .. } => Safety::Risky,
        }
    }

    /// The table this change applies to.
    pub fn table(&self) -> &str {
        match self {
            Self::CreateTable(table) => &table.name,
            Self::DropTable { name } => name,
            Self::AddColumn { table, .. }
            | Self::DropColumn { table, .. }
            | Self::AlterColumn { table, .. }
            | Self::AddIndex { table, .. }
            | Self::DropIndex { table, .. }
            | Self::AddForeignKey { table, .. }
            | Self::DropForeignKey { table, .. } => table,
        }
    }
}

impl core::fmt::Display for Change {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CreateTable(t) => write!(f, "create table {}", t.name),
            Self::DropTable { name } => write!(f, "drop table {name}"),
            Self::AddColumn { table, column } => {
                write!(f, "add column {table}.{}", column.name)
            }
            Self::DropColumn { table, column } => write!(f, "drop column {table}.{column}"),
            Self::AlterColumn { table, to, .. } => {
                write!(f, "alter column {table}.{}", to.name)
            }
            Self::AddIndex { table, index } => write!(f, "create index {} on {table}", index.name),
            Self::DropIndex { table, index } => write!(f, "drop index {} on {table}", index.name),
            Self::AddForeignKey { table, foreign_key } => {
                write!(f, "add foreign key {} on {table}", foreign_key.name)
            }
            Self::DropForeignKey { table, foreign_key } => {
                write!(f, "drop foreign key {} on {table}", foreign_key.name)
            }
        }
    }
}

/// Compute the changes needed to turn `current` into `desired`.
///
/// The returned list is already in a safe application order: tables are created
/// parents-first, and dropped children-first. Nothing is ever renamed automatically —
/// a rename looks exactly like a drop plus an add, so Besu refuses to guess. Declare the
/// rename explicitly with a migration if you want to keep the data.
pub fn diff(current: &Schema, desired: &Schema) -> Vec<Change> {
    let mut changes = Vec::new();

    // Create tables that are missing, in dependency order.
    for table in desired.topological_order() {
        if current.table(&table.name).is_none() {
            changes.push(Change::CreateTable(table.clone()));
        }
    }

    // Modify tables that exist on both sides.
    for table in &desired.tables {
        let Some(existing) = current.table(&table.name) else {
            continue;
        };
        diff_table(existing, table, &mut changes);
    }

    // Drop tables that are no longer wanted, children first.
    let ordered = current.topological_order();
    for table in ordered.into_iter().rev() {
        if desired.table(&table.name).is_none() {
            changes.push(Change::DropTable {
                name: table.name.clone(),
            });
        }
    }

    // Order within a table: additive first, then destructive.
    changes.sort_by_key(|change| match change {
        Change::CreateTable(_) => 0,
        Change::AddColumn { .. } => 1,
        Change::AlterColumn { .. } => 2,
        Change::AddIndex { .. } => 3,
        Change::AddForeignKey { .. } => 4,
        Change::DropForeignKey { .. } => 5,
        Change::DropIndex { .. } => 6,
        Change::DropColumn { .. } => 7,
        Change::DropTable { .. } => 8,
    });

    changes
}

fn diff_table(current: &Table, desired: &Table, changes: &mut Vec<Change>) {
    // Added columns.
    for column in &desired.columns {
        if current.column(&column.name).is_none() {
            changes.push(Change::AddColumn {
                table: desired.name.clone(),
                column: column.clone(),
            });
        }
    }

    // Removed columns.
    for column in &current.columns {
        if desired.column(&column.name).is_none() {
            changes.push(Change::DropColumn {
                table: desired.name.clone(),
                column: column.name.clone(),
            });
        }
    }

    // Changed columns.
    for column in &desired.columns {
        let Some(existing) = current.column(&column.name) else {
            continue;
        };
        if !existing.same_shape(column) {
            changes.push(Change::AlterColumn {
                table: desired.name.clone(),
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
                    table: desired.name.clone(),
                    index: index.clone(),
                });
                changes.push(Change::AddIndex {
                    table: desired.name.clone(),
                    index: index.clone(),
                });
            }
            None => changes.push(Change::AddIndex {
                table: desired.name.clone(),
                index: index.clone(),
            }),
        }
    }
    for index in &current.indexes {
        if !desired.indexes.iter().any(|i| i.name == index.name) {
            changes.push(Change::DropIndex {
                table: desired.name.clone(),
                index: index.clone(),
            });
        }
    }

    // Foreign keys.
    for fk in &desired.foreign_keys {
        match current.foreign_keys.iter().find(|f| f.name == fk.name) {
            Some(existing) if existing == fk => {}
            Some(_) => {
                changes.push(Change::DropForeignKey {
                    table: desired.name.clone(),
                    foreign_key: fk.clone(),
                });
                changes.push(Change::AddForeignKey {
                    table: desired.name.clone(),
                    foreign_key: fk.clone(),
                });
            }
            None => changes.push(Change::AddForeignKey {
                table: desired.name.clone(),
                foreign_key: fk.clone(),
            }),
        }
    }
    for fk in &current.foreign_keys {
        if !desired.foreign_keys.iter().any(|f| f.name == fk.name) {
            changes.push(Change::DropForeignKey {
                table: desired.name.clone(),
                foreign_key: fk.clone(),
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
