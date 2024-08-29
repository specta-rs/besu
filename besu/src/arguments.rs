use std::error;

use crate::Driver;

/// A trait for encoding arguments into a driver-specific format.
pub trait EncodeArgument<'a, T> {
    /// Encode the given value into the driver-specific format.
    fn encode(&mut self, value: T) -> Result<(), Box<dyn error::Error + Send + Sync + 'static>>
    where
        T: 'a;
}

// TODO: We should probs seal this?
/// A trait for decoding a generic set of SQL arguments into a driver-specific format.
pub trait Arguments<'a, D: Driver> {
    /// Convert the arguments into the driver-specific format.
    fn into_driver_arguments(
        self,
    ) -> Result<D::Arguments<'a>, Box<dyn error::Error + Send + Sync + 'static>>;
}

macro_rules! impl_tuple {
    ( impl $($i:ident),* ) => {
        #[allow(non_snake_case, unused_mut)]
        impl<'a, D: Driver, $($i: 'a),*> Arguments<'a, D> for ($($i,)*)
        where
            $(D::Arguments<'a>: EncodeArgument<'a, $i>),*
        {
            fn into_driver_arguments(self) -> Result<D::Arguments<'a>, Box<dyn error::Error + Send + Sync + 'static>> {
                let ( $( $i, )* ) = self;
                let mut args = D::Arguments::default();
                // TODO: Error handling
                $(<D::Arguments<'a> as EncodeArgument<'a, $i>>::encode(&mut args, $i)?;)*
                Ok(args)
            }
        }
    };
    ( $i2:ident $(, $i:ident)* ) => {
        impl_tuple!(impl $($i),* );
        impl_tuple!($($i),*);
    };
    () => {};
}

// TODO: We should possibly bump this limit under a feature flag???
impl_tuple!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13); // Technically we only support 12-tuples but the `T13` is required due to how the macro works
