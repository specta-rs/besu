use crate::Driver;

// TODO: Making this generic on value type or driver??? Props the former right?
pub trait Encode<D: Driver> {
    fn encode(&self) -> D::Value;
}
