// TODO: hide inners of macro like `serde_json::json`

/// TODO
#[macro_export]
macro_rules! select {
    ( $t:ident { $($k:ident : $v:expr),*  $(,)? } ) => {{
        // #[allow(non_camel_case_types)]

        // TODO: Factor out into runtime
        fn infer<T: $crate::IntoSql>(t: &T) -> T::Type {
            todo!();
        }


        $(let $k = { $v };)*

        // This asserts the expressions types match the provided struct.
        // It never actually runs!
        #[allow(unreachable_code, unused_must_use)]
        {
            $(let $k = &$k;)*
            || {
                let _ = $t {
                    $($k: infer($k)),*
                };
            };
        }

        struct Select {
            // TODO: Ensure these are properly sealed
            $($k: $crate::Sql<()>,)*
        }

        impl $crate::Selection for Select {
            type T = $t;

            fn arguments(self) -> Vec<$crate::Sql<()>> {
                vec![ $(self.$k),* ]
            }

            fn todo(&self) -> &'static str {
                concat!( $(stringify!($k)),* )
            }

            #[allow(unreachable_code)] // TODO: Remove this
            fn from_sql() -> Self::T {
                $t {
                    $($k: todo!()),*
                }
            }
        }

        fn into_sql<T: $crate::IntoSql>(t: T) -> $crate::Sql<()> {
            t.into_sql().erase()
        }

        Select {
            $($k: into_sql($k),)*
        }
    }};
    ( { $($k:ident : $v:expr),*  $(,)? } ) => {{
        struct AnonymousSelect<$($k),*> {
            // TODO: Ensure these are properly sealed
            $($k: $crate::Sql<$k>,)*
        }

        impl<$($k: $crate::IntoSql),*> $crate::Selection for AnonymousSelect<$($k),*> {
            type T = Self;

            fn arguments(self) -> Vec<$crate::Sql<()>> {
                vec![ $(self.$k.erase()),* ]
            }

            fn todo(&self) -> &'static str {
                concat!( $(stringify!($k)),* )
            }

            fn from_sql() -> Self::T {
                Self {
                    $($k: todo!()),*
                }
            }
        }

        fn into_sql<T: $crate::IntoSql>(t: T) -> $crate::Sql<<T as $crate::IntoSql>::Type> {
            t.into_sql()
        }

        AnonymousSelect {
            $($k: into_sql($v),)*
        }
    }};
}

// struct SelectData {
//     // $($k: <$k as $crate::IntoSql>::Type,)*
// }

// impl $crate::FromSql for SelectData {
//     fn from_sql() -> Self {
//         SelectData {
//             $($k: todo!(),)*
//         }
//     }
// }

// let todo: $t = $t {
//     $($k: todo!(),)*
// };

// // TODO: <$($k),*>
// struct SelectData {
//     // $($k: <$k as $crate::IntoSql>::Type,)*
// }

// TODO: $( => as $ty:expr )?
// ({ $($k:ident : $v:expr),*  $(,)? } $( as $ty:expr )?) => {{
//     $(
//         type $k = impl $crate::IntoSql;
//         let $k: $k = $v;
//     )*

//     // TODO: <$($k),*>
//     pub struct SelectData {
//         $($k: <$k as $crate::IntoSql>::Type,)*
//     }

//     let todo: SelectData = todo!();
//     todo
// }};
