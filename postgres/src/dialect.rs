use std::borrow::Cow;

use besu::{DeleteOperation, Dialect, InsertOperation, SelectOperation, UpdateOperation};

/// Postgres dialect implementation.
#[derive(Debug)]
pub struct Postgres;

impl Dialect for Postgres {
    fn positional_param(idx: usize) -> Cow<'static, str> {
        format!("${}", idx + 1).into()
    }

    fn select<V: Default>(op: &SelectOperation) -> (String, V) {
        // TODO: Don't use `*`, list all columns!
        let q = format!(r#"SELECT * FROM "{}""#, op.table);

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

        (q, Default::default())
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
