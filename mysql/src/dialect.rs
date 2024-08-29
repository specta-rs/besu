use std::borrow::Cow;

use besu::{Dialect, SelectOperation};

#[derive(Debug)]
pub struct MySql;

impl Dialect for MySql {
    fn positional_param(idx: usize) -> Cow<'static, str> {
        todo!()
    }

    fn select<V>(op: &SelectOperation) -> (String, Vec<V>) {
        todo!()
    }

    fn insert<V>(op: &besu::InsertOperation) -> (String, Vec<V>) {
        todo!()
    }

    fn update<V>(op: &besu::UpdateOperation) -> (String, Vec<V>) {
        todo!()
    }

    fn delete<V>(op: &besu::DeleteOperation) -> (String, Vec<V>) {
        todo!()
    }
}
