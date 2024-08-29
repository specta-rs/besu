use std::borrow::Cow;

use sqlx::{
    encode::IsNull,
    sqlite::{SqliteArgumentValue, SqliteTypeInfo},
};

pub enum SqliteArgument<'q> {
    Null,
    Text(Cow<'q, str>),
    Blob(Cow<'q, [u8]>),
    Double(f64),
    Int(i32),
    Int64(i64),
}

impl<'q, T: sqlx::Encode<'q, sqlx::Sqlite>> From<T> for SqliteArgument<'q> {
    fn from(value: T) -> Self {
        // TODO: Using a `Vec` like this is gonna be inefficient
        let mut args: Vec<SqliteArgumentValue> = Vec::new();
        <T as sqlx::Encode<'_, sqlx::Sqlite>>::encode(value, &mut args).unwrap(); // TODO: Error handling + null handling
        match args.into_iter().next().expect("trust me") {
            SqliteArgumentValue::Null => Self::Null,
            SqliteArgumentValue::Text(value) => Self::Text(value),
            SqliteArgumentValue::Blob(value) => Self::Blob(value),
            SqliteArgumentValue::Double(value) => Self::Double(value),
            SqliteArgumentValue::Int(value) => Self::Int(value),
            SqliteArgumentValue::Int64(value) => Self::Int64(value),
        }
    }
}

pub(crate) struct CompatSqliteArgument<'q>(pub &'q SqliteArgument<'q>);

impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for CompatSqliteArgument<'q> {
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, sqlx::error::BoxDynError> {
        match &self.0 {
            SqliteArgument::Null => Ok(IsNull::Yes),
            SqliteArgument::Text(value) => {
                println!("text");
                <&Cow<'q, str> as sqlx::Encode<'_, sqlx::Sqlite>>::encode(value, buf)
            }
            SqliteArgument::Blob(value) => {
                todo!();
                // <&Cow<'q, [u8]> as sqlx::Encode<'_, sqlx::Sqlite>>::encode(value, buf)
            }
            SqliteArgument::Double(value) => {
                <f64 as sqlx::Encode<'_, sqlx::Sqlite>>::encode(*value, buf)
            }
            SqliteArgument::Int(value) => {
                println!("int");
                <i32 as sqlx::Encode<'_, sqlx::Sqlite>>::encode(*value, buf)
            }
            SqliteArgument::Int64(value) => {
                println!("int64");
                <i64 as sqlx::Encode<'_, sqlx::Sqlite>>::encode(*value, buf)
            }
        }
    }
}

impl sqlx::Type<sqlx::Sqlite> for CompatSqliteArgument<'_> {
    fn type_info() -> SqliteTypeInfo {
        // Not used by SQLite`
        unreachable!();
    }

    fn compatible(_: &SqliteTypeInfo) -> bool {
        // Not used by SQLite`
        unreachable!();
    }
}
