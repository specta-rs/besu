//! Derive macros for [Besu](https://docs.rs/besu).
//!
//! You normally do not depend on this crate directly: enable the `derive` feature on `besu`
//! and write `#[derive(besu::Table)]`.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod sql_enum;
mod table;

/// Turn a struct into a database table.
///
/// The derive generates, from one plain struct:
///
/// - the row type itself (decoding implemented for every driver),
/// - a zero-sized table handle with one typed field per column, so `users.id` works,
/// - an insert struct with a constructor that requires exactly the columns the database
///   cannot fill in for you,
/// - the static [`TableDef`](https://docs.rs/besu-core/latest/besu_core/struct.TableDef.html)
///   metadata that powers queries, migrations and the CLI.
///
/// ```ignore
/// #[derive(besu::Table)]
/// #[besu(table = "users")]
/// pub struct User {
///     #[besu(primary_key, auto_increment)]
///     pub id: i64,
///     #[besu(unique)]
///     pub email: String,
///     pub name: String,
///     pub age: i32,
///     pub bio: Option<String>,
/// }
///
/// // `users` is generated from the table name.
/// db.select(users).filter(users.age.gt(18));
/// db.insert(users).value(users.insert("Oscar".into(), 21));
/// ```
///
/// # Attributes
///
/// On the struct:
///
/// - `#[besu(table = "name")]` — the SQL table name. Defaults to the struct name in
///   `snake_case`.
/// - `#[besu(schema = "name")]` — a schema/namespace, meaningful to PostgreSQL.
/// - `#[besu(index(columns(...)))]` — a table-level index. `columns` takes columns and raw
///   expressions, so `columns(user_id, lower(email))` is a composite index over the first
///   and an expression over the second. The other keys are `name = "..."`, `unique`,
///   `using = "gin"` (or `btree`, `gist`, `brin`, `fulltext`, `spatial`) and an operator
///   class: `opclass = "text_pattern_ops"` for a single entry, or
///   `opclass(email = "text_pattern_ops")` by column name. Without `name`, a composite index
///   is called `{table}_{columns}_idx` and an expression entry contributes `expr{position}`.
///   May be repeated.
/// - `#[besu(check = "expression")]` — a table-level `CHECK` constraint. An explicitly named
///   form is `#[besu(check(name = "...", expression = "..."))]`. Without a name the check is
///   called `{table}_{i}_check`, where `i` is its zero-based position among the checks on the
///   struct; the name is written explicitly into the DDL, so introspection reads it back.
///   May be repeated, including both spellings on one struct.
///
/// On a field:
///
/// - `#[besu(primary_key)]`
/// - `#[besu(auto_increment)]`
/// - `#[besu(unique)]`
/// - `#[besu(column = "name")]` — the SQL column name. Defaults to the field name.
/// - `#[besu(default = <literal>)]` — a literal default. Also `default_sql = "expr"` for an
///   expression and `default_now` for `CURRENT_TIMESTAMP`.
/// - `#[besu(generated = "expr")]` — a column the database computes from the others. It is
///   never written by an insert or an update. `stored = false` asks for a virtual column.
/// - `#[besu(references = "table.column")]` — declare a foreign key. Combine with
///   `on_delete = "cascade"` and `on_update = "..."`.
/// - `#[besu(index)]` — create a single-column index.
/// - `#[besu(skip)]` — not a column. The field must implement `Default`.
#[proc_macro_derive(Table, attributes(besu))]
pub fn table(input: TokenStream) -> TokenStream {
    match table::expand(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Turn a Rust enum into a SQL enum.
///
/// The enum's variants are the labels a column of this type accepts, so a MySQL column is
/// declared `ENUM('happy','meh','sad')`, a PostgreSQL column uses a named type created with
/// `CREATE TYPE`, and a SQLite column is `TEXT` holding the label. The same declaration
/// works on every dialect.
///
/// ```ignore
/// #[derive(Clone, Debug, PartialEq, Eq, besu::SqlEnum)]
/// #[besu(rename_all = "snake_case")]
/// pub enum Mood {
///     Happy,
///     #[besu(rename = "meh")]
///     Neutral,
///     Sad,
/// }
///
/// #[derive(Table)]
/// pub struct Review {
///     #[besu(primary_key, auto_increment)]
///     pub id: i64,
///     #[besu(sql_type = "Mood")]
///     pub mood: Mood,
/// }
/// ```
///
/// The derive implements [`SqlType`](https://docs.rs/besu/latest/besu/trait.SqlType.html),
/// `From<Mood> for Value`, `IntoSqlValue<Mood>` and `Decode` for every driver, so the field
/// can be inserted, compared and read back. The enum itself must also derive `Clone`,
/// because a whole-model insert clones the value.
///
/// # Attributes
///
/// On the enum:
///
/// - `#[besu(name = "mood")]` — the SQL type name. Defaults to the enum's name in
///   `snake_case`. PostgreSQL creates a type with this name; MySQL and SQLite ignore it.
/// - `#[besu(rename_all = "...")]` — how variant names become labels. One of `snake_case`,
///   `camelCase`, `PascalCase`, `SCREAMING_SNAKE_CASE`, `lowercase`, `UPPERCASE` or
///   `kebab-case`. Without it a label is the variant name exactly as written.
///
/// On a variant:
///
/// - `#[besu(rename = "meh")]` — the label for this variant, overriding `rename_all`.
///
/// An empty enum, a variant that holds data, a duplicate label and an unknown rename rule
/// are compile errors.
#[proc_macro_derive(SqlEnum, attributes(besu))]
pub fn sql_enum(input: TokenStream) -> TokenStream {
    match sql_enum::expand(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
