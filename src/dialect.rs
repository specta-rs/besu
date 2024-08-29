mod mysql;
mod sqlite;

use std::borrow::Cow;

pub use mysql::MySql;
pub use sqlite::SQLite;

use crate::{Driver, Select, SqlValue, Model};

pub trait Dialect {
    fn name() -> &'static str;

    fn positional_param(idx: usize) -> Cow<'static, str>;

    fn select<D: Driver, T: Model>(select: Select<D, T>) -> (String, Vec<SqlValue>);
}

// TODO: Assert `Dialect` is dyn safe. -> Do we care????
