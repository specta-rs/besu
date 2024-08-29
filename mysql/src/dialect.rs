use std::borrow::Cow;

use besu::{DeleteOperation, Dialect, InsertOperation, SelectOperation, UpdateOperation};

/// MySQL dialect implementation.
#[derive(Debug)]
pub struct MySQL;

impl Dialect for MySQL {
    fn positional_param(_: usize) -> Cow<'static, str> {
        Cow::Borrowed("?")
    }

    fn select<V: Default>(op: &SelectOperation) -> (String, V) {
        // TODO: Don't use `*`, list all columns!
        let q = format!("SELECT * FROM `{}`", op.table);

        // for (i, (col, val)) in op.where_equals.iter().enumerate() {
        //     if i == 0 {
        //         q.push_str(" WHERE ");
        //     } else {
        //         q.push_str(" AND ");
        //     }
        //     q.push_str(col);
        //     q.push_str(" = ");
        //     q.push_str(Self::positional_param(i).as_ref());
        // }

        (q, V::default())
    }

    #[allow(clippy::todo)]
    fn insert<V>(_op: &InsertOperation) -> (String, V) {
        todo!()
    }

    #[allow(clippy::todo)]
    fn update<V>(_op: &UpdateOperation) -> (String, V) {
        todo!()
    }

    #[allow(clippy::todo)]
    fn delete<V>(_op: &DeleteOperation) -> (String, V) {
        todo!()
    }
}
