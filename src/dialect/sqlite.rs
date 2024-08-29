use std::{borrow::Cow, fmt};

use crate::{Driver, Model, Select, SqlValue};

use super::Dialect;

pub struct SQLite;

impl fmt::Debug for SQLite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SQLite") // TODO: Fix up this impl
    }
}

impl Dialect for SQLite {
    fn name() -> &'static str {
        "SQLite"
    }

    fn positional_param(idx: usize) -> Cow<'static, str> {
        Cow::Borrowed("?")
    }

    fn select<D: Driver, T: Model>(select: Select<D, T>) -> (String, Vec<SqlValue>) {
        // TODO: Don't use `*`, list all columns!

        (format!("SELECT * FROM {}", T::NAME), vec![])
    }
}
