use std::borrow::Cow;

use besu::Dialect;

/// SQLite dialect implementation.
#[derive(Debug)]
pub struct SQLite;

impl Dialect for SQLite {
    // fn positional_param(_: usize) -> Cow<'static, str> {
    //     Cow::Borrowed("?")
    // }

    // fn select(op: &SelectOperation) -> (String, Vec<InsertArgumentRef<'_>>) {
    //     // TODO: Don't use `*`, list all columns!
    //     let q = format!(r#"SELECT * FROM "{}""#, op.table);

    //     // for (i, (col, val)) in op.where_equals.iter().enumerate() {
    //     //     if i == 0 {
    //     //         q.push_str(" WHERE ");
    //     //     } else {
    //     //         q.push_str(" AND ");
    //     //     }
    //     //     q.push_str(col);
    //     //     q.push_str(" = ");
    //     //     q.push_str("?");
    //     // }

    //     (q, Default::default())
    // }

    // fn insert<'a>(op: &InsertOperation<'a>) -> (String, Vec<InsertArgumentRef<'a>>) {
    //     if op.values_len == 0 {
    //         panic!("No shot"); // TODO
    //     }

    //     let mut q = format!(r#"INSERT INTO "{}" ("#, op.table);
    //     for (i, col) in op.columns.iter().enumerate() {
    //         if i != 0 {
    //             q.push_str(", ");
    //         }
    //         q.push_str(col); // TODO: Escape values
    //     }
    //     q.push_str(") VALUES ");

    //     let mut args = Vec::default();
    //     for (i, columns) in op.values().enumerate() {
    //         if i != 0 {
    //             q.push_str(", ");
    //         }
    //         q.push_str("(");

    //         for (i, (_, arg)) in columns.enumerate() {
    //             if i != 0 {
    //                 q.push_str(", ");
    //             }

    //             q.push_str("?");
    //             args.push(arg);
    //         }
    //         q.push_str(")");
    //     }

    //     println!("{q:?}"); // TODO

    //     (q, args)
    // }

    // #[allow(clippy::todo)]
    // fn update<V>(_op: &UpdateOperation) -> (String, V) {
    //     todo!()
    // }

    // #[allow(clippy::todo)]
    // fn delete<V>(_op: &DeleteOperation) -> (String, V) {
    //     todo!()
    // }
}
