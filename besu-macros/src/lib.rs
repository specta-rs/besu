//! Derive macros for [Besu](https://docs.rs/besu).
//!
//! You normally do not depend on this crate directly: enable the `derive` feature on `besu`
//! and write `#[derive(besu::Table)]`.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

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
///
/// On a field:
///
/// - `#[besu(primary_key)]`
/// - `#[besu(auto_increment)]`
/// - `#[besu(unique)]`
/// - `#[besu(column = "name")]` — the SQL column name. Defaults to the field name.
/// - `#[besu(default = <literal>)]` — a literal default. Also `default_sql = "expr"` for an
///   expression and `default_now` for `CURRENT_TIMESTAMP`.
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
