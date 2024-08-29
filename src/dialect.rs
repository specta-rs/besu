use std::borrow::Cow;

use crate::{Driver, Model, Select, SqlValue};

pub trait Dialect {
    fn name() -> &'static str;

    fn positional_param(idx: usize) -> Cow<'static, str>;

    fn select<D: Driver, T: Model>(select: Select<D, T>) -> (String, Vec<SqlValue>);
}

// TODO: Assert `Dialect` is dyn safe. -> Do we care????
