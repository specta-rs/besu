use crate::Driver;

/// Decode a value, or a whole row, from a query result.
///
/// One trait covers both scalars and composite rows. A scalar consumes one column, a model
/// struct consumes one column per field, and a tuple consumes the sum. That uniformity is
/// what makes `Option<T>` work for both "this column was NULL" and "this outer-joined row
/// was absent": an `Option<T>` is `None` when every column `T` would consume is `NULL`.
///
/// Implementations for primitives live in each driver crate, because extracting a value
/// from a row is inherently database specific.
pub trait Decode<D: Driver>: Sized {
    /// How many columns this type consumes.
    const COLUMNS: usize = 1;

    /// Decode starting at `*offset`, advancing it by [`Self::COLUMNS`].
    fn decode(row: &D::Row, offset: &mut usize) -> Result<Self, D::Error>;
}

impl<D: Driver> Decode<D> for () {
    const COLUMNS: usize = 0;

    fn decode(_: &D::Row, _: &mut usize) -> Result<Self, D::Error> {
        Ok(())
    }
}

impl<D: Driver, T: Decode<D>> Decode<D> for Option<T> {
    const COLUMNS: usize = T::COLUMNS;

    fn decode(row: &D::Row, offset: &mut usize) -> Result<Self, D::Error> {
        let start = *offset;
        let mut all_null = true;
        for index in start..start + T::COLUMNS {
            if !D::is_null(row, index) {
                all_null = false;
                break;
            }
        }

        if all_null {
            *offset += T::COLUMNS;
            Ok(None)
        } else {
            T::decode(row, offset).map(Some)
        }
    }
}

macro_rules! impl_tuple {
    ($($name:ident),+) => {
        impl<D: Driver, $($name: Decode<D>),+> Decode<D> for ($($name,)+) {
            const COLUMNS: usize = 0 $(+ $name::COLUMNS)+;

            #[allow(non_snake_case)]
            fn decode(row: &D::Row, offset: &mut usize) -> Result<Self, D::Error> {
                $(let $name = $name::decode(row, offset)?;)+
                Ok(($($name,)+))
            }
        }
    };
}

impl_tuple!(A);
impl_tuple!(A, B);
impl_tuple!(A, B, C);
impl_tuple!(A, B, C, D4);
impl_tuple!(A, B, C, D4, D5);
impl_tuple!(A, B, C, D4, D5, D6);
impl_tuple!(A, B, C, D4, D5, D6, D7);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9, D10);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9, D10, D11);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9, D10, D11, D12);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9, D10, D11, D12, D13);
impl_tuple!(A, B, C, D4, D5, D6, D7, D8, D9, D10, D11, D12, D13, D14);
impl_tuple!(
    A, B, C, D4, D5, D6, D7, D8, D9, D10, D11, D12, D13, D14, D15
);
impl_tuple!(
    A, B, C, D4, D5, D6, D7, D8, D9, D10, D11, D12, D13, D14, D15, D16
);
