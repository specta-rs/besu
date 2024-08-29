use std::{borrow::Cow, fmt};

use besu::{Dialect, Driver, Model, Select, SqlValue};

pub struct MySql;

impl fmt::Debug for MySql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MySQL")
    }
}

impl Dialect for MySql {
    fn name() -> &'static str {
        "MySQL"
    }

    fn positional_param(idx: usize) -> Cow<'static, str> {
        todo!()
    }

    fn select<D: Driver, T: Model>(select: Select<D, T>) -> (String, Vec<SqlValue>) {
        todo!()
    }
}
