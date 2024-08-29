use std::{error, future::Future};

use crate::{Dialect, ExecutedQuery, SqlValue};

pub trait Driver: 'static {
    const NAME: &'static str;

    type Error: error::Error;
    type Dialect: Dialect;

    fn execute(
        &self,
        sql: &str,
        values: &[SqlValue],
    ) -> impl Future<Output = Result<ExecutedQuery, Self::Error>>;
}
