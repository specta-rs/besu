use besu_core::{ColumnDef, DefaultValue, TableDef, TypeDef};
use serde::{Deserialize, Serialize};

/// A complete database schema.
///
/// Tables are kept in a `Vec` rather than a map so that iteration order — and therefore
/// generated DDL — is deterministic. Lookup is linear, which is irrelevant for schemas of
/// even a thousand tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// Every table in the schema.
    #[serde(default)]
    pub tables: Vec<Table>,
    /// Schema-level named enums, kept for round-tripping databases that have them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<Enum>,
}

/// A named enum type, as found in PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enum {
    /// The type name.
    pub name: String,
    /// The allowed values, in order.
    pub values: Vec<String>,
}

impl Schema {
    /// An empty schema.
    pub const fn new() -> Self {
        Self {
            tables: Vec::new(),
            enums: Vec::new(),
        }
    }

    /// Build a schema from the static metadata emitted by `#[derive(Table)]`.
    pub fn from_defs(defs: &[&'static TableDef]) -> Self {
        let mut schema = Self::new();
        for def in defs {
            schema.push_def(def);
        }
        schema.tables.sort_by(|a, b| a.name.cmp(&b.name));
        schema
    }

    /// Add (or replace) a table from static metadata.
    pub fn push_def(&mut self, def: &TableDef) {
        let table = Table::from_def(def);
        if let Some(slot) = self.tables.iter_mut().find(|t| t.name == table.name) {
            *slot = table;
        } else {
            self.tables.push(table);
        }
    }

    /// Find a table by name.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// Find a table by name, mutably.
    pub fn table_mut(&mut self, name: &str) -> Option<&mut Table> {
        self.tables.iter_mut().find(|t| t.name == name)
    }

    /// Sort tables by name so output is stable.
    pub fn sort(&mut self) {
        self.tables.sort_by(|a, b| a.name.cmp(&b.name));
        for table in &mut self.tables {
            table.normalise();
        }
    }

    /// Order tables so that a table always comes after the tables it references.
    ///
    /// Cycles (which are legal SQL when the constraint is added afterwards) are broken
    /// arbitrarily rather than reported as an error.
    pub fn topological_order(&self) -> Vec<&Table> {
        fn visit<'a>(
            table: &'a Table,
            schema: &'a Schema,
            seen: &mut Vec<&'a str>,
            out: &mut Vec<&'a Table>,
        ) {
            if seen.contains(&table.name.as_str()) {
                return;
            }
            seen.push(table.name.as_str());
            for fk in &table.foreign_keys {
                if fk.references_table == table.name {
                    continue;
                }
                if let Some(dep) = schema.table(&fk.references_table) {
                    visit(dep, schema, seen, out);
                }
            }
            out.push(table);
        }

        let mut out = Vec::with_capacity(self.tables.len());
        let mut seen = Vec::with_capacity(self.tables.len());
        for table in &self.tables {
            visit(table, self, &mut seen, &mut out);
        }
        out
    }
}

/// A single table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// The SQL table name.
    pub name: String,
    /// An optional schema/namespace, meaningful to PostgreSQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Columns in declaration order.
    pub columns: Vec<Column>,
    /// Secondary indexes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    /// Foreign key constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    /// Tables of named enums are rare; kept for fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl Table {
    /// Build from the static metadata emitted by the derive.
    pub fn from_def(def: &TableDef) -> Self {
        Self {
            name: def.name.into(),
            schema: def.schema.map(Into::into),
            columns: def.columns.iter().map(Column::from_def).collect(),
            indexes: def
                .indexes
                .iter()
                .map(|idx| Index {
                    name: idx.name.into(),
                    columns: idx.columns.iter().map(|c| (*c).into()).collect(),
                    unique: idx.unique,
                })
                .collect(),
            foreign_keys: def
                .foreign_keys
                .iter()
                .map(|fk| ForeignKey {
                    name: fk.name.into(),
                    columns: fk.columns.iter().map(|c| (*c).into()).collect(),
                    references_table: fk.references_table.into(),
                    references_columns: fk.references_columns.iter().map(|c| (*c).into()).collect(),
                    on_delete: fk.on_delete.into(),
                    on_update: fk.on_update.into(),
                })
                .collect(),
            comment: None,
        }
    }

    /// Find a column by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The primary key columns, in declaration order.
    pub fn primary_key(&self) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }

    /// Whether this table has no explicit primary key.
    pub fn has_primary_key(&self) -> bool {
        self.columns.iter().any(|c| c.primary_key)
    }

    /// Sort columns so the primary key comes first and the rest keep their order.
    ///
    /// A stable order matters: diffing two schemas with different column order should not
    /// report changes, and this gives every producer of a [`Table`] a canonical layout.
    pub fn normalise(&mut self) {
        self.columns.sort_by_key(|c| !c.primary_key);
        self.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        self.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// A single column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// The SQL column name.
    pub name: String,
    /// The canonical type.
    pub ty: TypeDef,
    /// Whether the column accepts `NULL`.
    #[serde(default)]
    pub nullable: bool,
    /// Whether the database generates values for this column.
    #[serde(default)]
    pub auto_increment: bool,
    /// Whether the column is part of the primary key.
    #[serde(default)]
    pub primary_key: bool,
    /// Whether the column has its own `UNIQUE` constraint.
    #[serde(default)]
    pub unique: bool,
    /// The default, as a normalised SQL expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// An optional comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl Column {
    /// Build from the static metadata emitted by the derive.
    pub fn from_def(def: &ColumnDef) -> Self {
        Self {
            name: def.name.into(),
            ty: def.ty.clone(),
            nullable: def.nullable,
            auto_increment: def.auto_increment,
            primary_key: def.primary_key,
            unique: def.unique,
            default: def.default.map(default_to_sql),
            comment: None,
        }
    }

    /// Whether two columns describe the same thing, ignoring name.
    pub(crate) fn same_shape(&self, other: &Self) -> bool {
        normalise_type(&self.ty) == normalise_type(&other.ty)
            && self.nullable == other.nullable
            && self.auto_increment == other.auto_increment
            && self.primary_key == other.primary_key
            && self.unique == other.unique
            && normalise_default(self.default.as_deref())
                == normalise_default(other.default.as_deref())
    }
}

/// A secondary index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// The index name.
    pub name: String,
    /// The indexed columns, in order.
    pub columns: Vec<String>,
    /// Whether the index is unique.
    #[serde(default)]
    pub unique: bool,
}

/// A foreign key constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// The constraint name.
    pub name: String,
    /// Columns on this table.
    pub columns: Vec<String>,
    /// The referenced table.
    pub references_table: String,
    /// The referenced columns.
    pub references_columns: Vec<String>,
    /// Behaviour on delete.
    #[serde(default)]
    pub on_delete: ReferenceAction,
    /// Behaviour on update.
    #[serde(default)]
    pub on_update: ReferenceAction,
}

pub use besu_core::schema::ReferenceAction;

/// A default value, normalised to a SQL expression for comparison.
pub type Default = Option<String>;

fn default_to_sql(value: DefaultValue) -> String {
    value.render().into_owned()
}

/// Fold equivalent spellings of the same type together.
///
/// Databases disagree wildly here: SQLite calls everything `INTEGER`, MySQL reports `int(11)`
/// for `INT`, and Postgres reports `character varying(255)` for `VARCHAR(255)`. Comparing
/// canonical types avoids a diff that never converges.
pub fn normalise_type(ty: &TypeDef) -> TypeDef {
    match ty {
        TypeDef::Char(_) | TypeDef::VarChar(_) => TypeDef::Text,
        TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 => TypeDef::Int8,
        TypeDef::Json | TypeDef::Jsonb => TypeDef::Json,
        other => other.clone(),
    }
}

/// Fold equivalent spellings of the same default together.
pub fn normalise_default(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_matches('\'').trim();
    let lowered = value.to_ascii_lowercase();
    let lowered = match lowered.as_str() {
        "now()" | "current_timestamp()" | "current_timestamp" => "current_timestamp",
        "" => return None,
        _ => return Some(lowered),
    };
    Some(lowered.into())
}

#[cfg(feature = "fs")]
mod fs_impl {
    use super::Schema;
    use std::path::Path;

    impl Schema {
        /// Read a schema snapshot from a JSON file.
        pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
            let contents = std::fs::read_to_string(path)?;
            serde_json::from_str(&contents)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
        }

        /// Write this schema to a JSON snapshot file.
        pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
            let json = serde_json::to_string_pretty(self)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            std::fs::write(path, json)
        }
    }
}
