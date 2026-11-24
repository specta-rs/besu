//! The pgvector binary format has a signed 16-bit dimension. The sqlx encoder must reject a
//! longer value before it writes a truncated argument.

#![cfg(feature = "sqlx")]
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::{IntoSqlValue, Sql, Value, VectorOf};
use besu_postgres::{Postgres, encode_arguments};

#[test]
fn sqlx_rejects_an_oversized_typed_vector() {
    let components = (0..32_768).map(|value| value as f32).collect::<Vec<_>>();
    let value = <Vec<f32> as IntoSqlValue<VectorOf<32_767>>>::into_sql_value(components);
    let Value::Vector(vector) = value else {
        panic!("a typed vector conversion produced another value");
    };
    assert_eq!(vector.len(), 32_768);

    let mut sql = Sql::<Postgres>::raw("SELECT $1");
    sql.push_arg(Value::Vector(vector));
    let error = encode_arguments(&sql.seal()).unwrap_err();
    assert!(error.to_string().contains("32767"), "{error}");
}
