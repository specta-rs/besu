use crate::Driver;

pub trait DecodeRow<D: Driver> {
    fn decode(row: D::Row) -> Self;
}

pub trait DecodeValue<D: Driver, T> {
    fn decode<'q>(value: D::Value<'q>) -> T;
}
