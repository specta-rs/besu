use besu::DecodeValue;
use sqlx::{sqlite::SqliteValueRef, ValueRef};

use crate::Sqlx;

// TODO: Implement all the types
impl DecodeValue<Sqlx> for i32 {
    fn decode<'a>(value: SqliteValueRef<'a>) -> Self {
        <Self as sqlx::Decode<'a, sqlx::Sqlite>>::decode(value).unwrap() // TODO: Error handling
    }
}

impl DecodeValue<Sqlx> for String {
    fn decode<'a>(value: SqliteValueRef<'a>) -> Self {
        <Self as sqlx::Decode<'a, sqlx::Sqlite>>::decode(value).unwrap() // TODO: Error handling
    }
}

impl<T: DecodeValue<Sqlx>> DecodeValue<Sqlx> for Option<T> {
    fn decode<'a>(value: SqliteValueRef<'a>) -> Self {
        if value.is_null() {
            None
        } else {
            Some(T::decode(value))
        }
    }
}

// TODO: `Box`, `Arc`, etc
// TODO: Rest of standard library types
