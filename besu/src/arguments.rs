use crate::Driver;

pub trait EncodeArgument<'a, T> {
    fn encode(&mut self, value: T)
    where
        T: 'a;
}

pub trait Arguments<'a, D: Driver> {
    fn into_driver_arguments(self) -> D::Arguments<'a>;
}

macro_rules! impl_tuple {
    ( impl $($i:ident),* ) => {
        #[allow(non_snake_case, unused_mut)]
        impl<'a, D: Driver, $($i: 'a),*> Arguments<'a, D> for ($($i,)*)
        where
            $(D::Arguments<'a>: EncodeArgument<'a, $i>),*
        {
            fn into_driver_arguments(self) -> D::Arguments<'a> {
                let ( $( $i, )* ) = self;
                let mut args = D::Arguments::default();
                $(<D::Arguments<'a> as EncodeArgument<'a, $i>>::encode(&mut args, $i);)*
                args
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
