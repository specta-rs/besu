//! Derive macros for [Besu](https://docs.rs/besu).
//!
//! You normally do not depend on this crate directly: enable the `derive` feature on `besu`
//! and write `#[derive(besu::Table)]`.
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

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
