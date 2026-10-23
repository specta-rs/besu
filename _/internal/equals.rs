// https://github.com/rust-lang/rust/issues/20041#issuecomment-820911297

pub trait TyEq {}

impl<T> TyEq for (T, T) {}
