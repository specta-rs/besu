use crate::Driver;

pub trait DecodeRow<D: Driver> {
    fn decode(row: D::Row) -> Self;
}

pub trait DecodeValue<D: Driver> {
    fn decode<'a>(value: D::Value<'a>) -> Self;
}
