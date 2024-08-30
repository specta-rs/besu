// TODO: This macro should ensure the correct number of args are supplied like `format!`
/// A macro for constructing an SQL query with escaped arguments.
///
/// TODO: Example usage
#[macro_export]
macro_rules! sql {
    ($query:literal $(, $arg:expr )* $(,)?) => {
        (
            {
                let query: &'static str = $query;
                query
            },
            ($($arg,)*),
        )
    };
}

/// A macro for constructing a selection of a [`Table`].
///
/// TODO: Example usage
#[macro_export]
macro_rules! select {
    // TODO: The error for a missing field is on the whole macro. Can we make it span to the specific field?
    // TODO: The error also links to the `derive(Table)` not the `select!` invocation. Can we fix that???

    // TODO: Can we trick the LSP into *any* hints
    // TODO: Support nested structs?? (Eg. { user: { }, post: { } })
    // TODO: `$m` not being a `path` is cringe!

    // TODO: Can we deduplicate some of this logic with the derive macros???

    ($s:ident, { $($k:ident: $m:ident . $f:ident),* $(,)? }) => {
        const _: () = {
            $( $m!(@verify; $f); )*
        };


        #[derive(Debug, Clone, Default)] // TODO: Can the user configure these somehow so we can avoid a custom Specta integration?
        pub struct $s {
            $(pub $k: $m!(@field; $f)),*
        }

        #[automatically_derived]
        impl<D: $crate::Driver> $crate::DecodeRow<D> for $s
        where
            $( <D as $crate::Driver>::ValueDecoder: $crate::DecodeValue<D, $m!(@field; $f)> ),*
        {
            fn decode(row: D::Row) -> ::std::result::Result<Self, ::std::boxed::Box<dyn ::std::error::Error + Send + Sync + 'static>>
            where
                Self: ::std::marker::Sized
            {
                let mut i = D::row_len(&row);
                if i != 0 { i -= 1 };
                Ok(Self {
                    $(
                        $k: {
                            let v = D::get_value(&row, i)?;
                            if i != 0 { i -= 1 };
                            <D::ValueDecoder as $crate::DecodeValue<D, $m!(@field; $f)>>::decode(v)?
                        }
                    ),*
                })
            }
        }

        // TODO: Hide these within a submodule

        mod fields {
            $(
                pub struct $k;

                #[automatically_derived]
                impl $crate::Column<super::Model> for $k {
                    const NAME: &'static str = stringify!($k); // TODO: This should come from whatever it is defined as on the root model

                    type Type = $m!(@field; $f);
                }
            )*
        }

        pub struct Model {
            ____besu_table_name: &'static str,
            $(pub $k: fields::$k),*
        }

        #[automatically_derived]
        impl Table for Model {
            fn name(&self) -> &'static str {
                self.____besu_table_name
            }

            fn selection(&self) -> &'static [&'static str] {
                &[
                    // $( <fields::$k as $crate::Column<_>>::NAME ),*
                ]
            }

            type Model = $s;
            type Insert = (); // TODO: We might wanna break this off
        }

        // TODO: Enforce `T` is contained in the selection???
        pub fn $s<T: Table>(from: T) -> Model {
            Model {
                ____besu_table_name: from.name(),
                $( $k: fields::$k ),*
            }
        }
    };
}
