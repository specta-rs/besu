use std::borrow::Cow;

use besu::{Dialect, SelectOperation};

#[derive(Debug)]
pub struct SQLite;

impl Dialect for SQLite {
    fn positional_param(idx: usize) -> Cow<'static, str> {
        Cow::Borrowed("?")
    }

    fn select<V>(op: &SelectOperation<V>) -> (String, Vec<V>) {
        // TODO: Don't use `*`, list all columns!
        let mut q = format!("SELECT * FROM {}", op.table);

        for (i, (col, val)) in op.where_equals.iter().enumerate() {
            if i == 0 {
                q.push_str(" WHERE ");
            } else {
                q.push_str(" AND ");
            }
            q.push_str(col);
            q.push_str(" = ");
            q.push_str(Self::positional_param(i).as_ref());
        }

        (q, vec![])
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
