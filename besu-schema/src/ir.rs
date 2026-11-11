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
    ///
    /// Only PostgreSQL has types that are not part of a column. The differ creates these
    /// before the tables that use them and drops them after, and a column of an enum type
    /// is `TypeDef::Custom`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<Enum>,
    /// Views, kept for round-tripping only.
    ///
    /// A view is created by a migration, not by `#[derive(Table)]`, so the differ never
    /// reports one: a view that exists in the database but not here is left exactly as it
    /// is, and a view declared here is never created. Introspection fills this list so
    /// `besu pull` produces a snapshot that names what is really there; `push` ignores it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<View>,
}

/// A view, as reported by a database.
///
/// The definition is the dialect's own description of the view, verbatim: PostgreSQL and
/// MySQL report the `SELECT` body, while SQLite reports the whole `CREATE VIEW` statement.
/// Nothing in Besu executes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The view's name, qualified by its schema the same way a [`Table`] is.
    pub name: String,
    /// The dialect's own description of the view.
    pub definition: String,
}

/// A named enum type, as found in PostgreSQL.
///
/// The name is unqualified for an enum in the database's default schema and `schema.name`
/// otherwise, the same rule [`Table`] follows.
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
            views: Vec::new(),
        }
    }

    /// Build a schema from the static metadata emitted by `#[derive(Table)]`.
    pub fn from_defs(defs: &[&'static TableDef]) -> Self {
        let mut schema = Self::new();
        for def in defs {
            schema.push_def(def);
        }
        schema.sort();
        schema
    }

    /// Add (or replace) a table from static metadata.
    pub fn push_def(&mut self, def: &TableDef) {
        let table = Table::from_def(def);
        if let Some(slot) = self
            .tables
            .iter_mut()
            .find(|t| t.schema == table.schema && t.name == table.name)
        {
            *slot = table;
        } else {
            self.tables.push(table);
        }
    }

    /// Find a table by name in the default schema.
    ///
    /// A table's identity is `(schema, name)`, so two tables called `users` in different
    /// schemas are distinct. This matches the unqualified table first; when there is no
    /// such table it falls back to the first table with that name, which keeps lookups by
    /// name working for schemas that were introspected explicitly. Use [`Schema::find`]
    /// when the schema matters.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.find(None, name)
            .or_else(|| self.tables.iter().find(|t| t.name == name))
    }

    /// Find a table by name in the default schema, mutably.
    pub fn table_mut(&mut self, name: &str) -> Option<&mut Table> {
        if self
            .tables
            .iter()
            .any(|t| t.schema.is_none() && t.name == name)
        {
            return self
                .tables
                .iter_mut()
                .find(|t| t.schema.is_none() && t.name == name);
        }
        self.tables.iter_mut().find(|t| t.name == name)
    }

    /// Find a table by its schema and name.
    ///
    /// `schema` is the table's schema, or `None` for the default schema. This is the
    /// identity the differ uses.
    pub fn find(&self, schema: Option<&str>, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|t| t.name == name && t.schema.as_deref() == schema)
    }

    /// Find a table by a qualified name, as carried by [`Change`](crate::Change).
    ///
    /// The name is `schema.table` when the table has a schema, and a bare `table`
    /// otherwise.
    pub fn get(&self, qualified: &str) -> Option<&Table> {
        match qualified.split_once('.') {
            Some((schema, name)) => self.find(Some(schema), name),
            None => self.find(None, qualified),
        }
    }

    /// Find a named enum type by name.
    pub fn find_enum(&self, name: &str) -> Option<&Enum> {
        self.enums.iter().find(|e| e.name == name)
    }

    /// Sort tables by schema and name, enums by name, and views by name, so output is stable.
    pub fn sort(&mut self) {
        self.tables
            .sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.name.cmp(&b.name)));
        self.enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.views.sort_by(|a, b| a.name.cmp(&b.name));
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
            seen: &mut Vec<&'a Table>,
            out: &mut Vec<&'a Table>,
        ) {
            if seen
                .iter()
                .any(|seen| seen.name == table.name && seen.schema == table.schema)
            {
                return;
            }
            seen.push(table);
            for fk in &table.foreign_keys {
                if fk.references_table == table.qualified_name() {
                    continue;
                }
                let referenced = schema
                    .get(&fk.references_table)
                    .or_else(|| schema.table(&fk.references_table));
                if let Some(dep) = referenced {
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
    /// Multi-column `UNIQUE` constraints.
    ///
    /// A single-column `UNIQUE` lives on the column itself, as [`Column::unique`]. A
    /// constraint over several columns has no per-column spelling and is a separate object
    /// the database owns, so it is kept here rather than being modelled as an index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uniques: Vec<Unique>,
    /// Foreign key constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    /// `CHECK` constraints, which the derive cannot declare and a snapshot can.
    ///
    /// A check is written inside `CREATE TABLE` and added or dropped with
    /// `ALTER TABLE ... ADD/DROP CONSTRAINT` on the dialects that support it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
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
                    // A `TableDef` has no way to spell a predicate, so a declared index is
                    // always a plain one.
                    predicate: None,
                })
                .collect(),
            // The derive has no attribute for a composite `UNIQUE`; a hand-written
            // `TableDef` cannot carry one either, so this is only ever populated by
            // introspection.
            uniques: Vec::new(),
            foreign_keys: def
                .foreign_keys
                .iter()
                .map(|fk| ForeignKey {
                    name: fk.name.into(),
                    columns: fk.columns.iter().map(|c| (*c).into()).collect(),
                    references_table: fk.references_table.into(),
                    references_columns: fk.references_columns.iter().map(|c| (*c).into()).collect(),
                    on_delete: fk.on_delete,
                    on_update: fk.on_update,
                })
                .collect(),
            // The derive has no attribute for a `CHECK` constraint, so this is only ever
            // populated by introspection or a hand-written snapshot.
            checks: Vec::new(),
            comment: None,
        }
    }

    /// The table's name, qualified by its schema when it has one, as in `app.users`.
    ///
    /// This is the identity [`Change`](crate::Change) carries and what
    /// [`Ddl::quote_table`](crate::Ddl::quote_table) expects; it is *not* quoted.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
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
        self.uniques.sort_by(|a, b| a.name.cmp(&b.name));
        self.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
        self.checks.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// A multi-column `UNIQUE` constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unique {
    /// The constraint name.
    pub name: String,
    /// The constrained columns, in order.
    pub columns: Vec<String>,
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
    /// The column's previous name, if the declaration says it was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
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
            renamed_from: def.renamed_from.map(Into::into),
        }
    }

    /// Whether two columns describe the same thing, ignoring name.
    ///
    /// `normalise` folds types the dialect cannot tell apart, so a database that has a
    /// single integer type does not diff forever against a schema that distinguishes them.
    pub(crate) fn same_shape(&self, other: &Self, normalise: &dyn Fn(&TypeDef) -> TypeDef) -> bool {
        normalise(&self.ty) == normalise(&other.ty)
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
    /// The `WHERE` clause of a partial index, if it has one.
    ///
    /// Only PostgreSQL has this read back into the IR. SQLite supports partial indexes but
    /// its introspection skips them rather than guess, and MySQL has none, so both leave
    /// this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

/// A `CHECK` constraint.
///
/// The expression is a raw SQL boolean expression. It is compared after folding away the
/// redundant parentheses PostgreSQL adds, so a check declared as `value > 0` converges
/// against the `((value > 0))` the database reports back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// The constraint name.
    pub name: String,
    /// The boolean expression inside `CHECK (...)`.
    pub expression: String,
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

/// Fold equivalent spellings of a `CHECK` expression together.
///
/// PostgreSQL stores `CHECK (value > 0)` and reports it as `(value > 0)`, with one pair of
/// parentheses for every level it was written at. Stripping pairs that wrap the whole
/// expression leaves the spelling a person would write, so a check declared in a snapshot
/// converges instead of being dropped and added on every push.
///
/// Only parentheses wrapping the *whole* expression are removed; `(a > 0) AND (b > 0)` keeps
/// both of its pairs.
pub fn normalise_expression(expression: &str) -> String {
    let mut current = expression.trim();
    while let Some(inner) = strip_outer_parens(current) {
        current = inner.trim();
    }
    current.to_owned()
}

/// The inside of `sql` when one pair of parentheses wraps all of it.
fn strip_outer_parens(sql: &str) -> Option<&str> {
    let inner = sql.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    let mut quote = 0u8;
    for byte in inner.bytes() {
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = byte,
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                if depth == 0 {
                    // The opening parenthesis closed before the end, so the outer pair does
                    // not wrap the whole expression.
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    Some(inner)
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
