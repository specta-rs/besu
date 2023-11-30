mod column;
mod model;
mod table;

pub use column::{Column, ColumnDefinition};
pub use model::Model;
pub use table::{Table, TableDefinition};

// TODO: Error on reserved field names - `insert`, etc

#[macro_export]
macro_rules! table {
    ($name:ident, $name_def:ident, { $($k:ident: $t:ty $(; $(@default($default:expr))? )?),* $(,)? }, {
        insert: { $($ik:ident: $it:ty),* $(,)? },
        insertOptional: { $($iok:ident: $iot:ty),* $(,)? }
    }) => {
        #[derive(Debug, Clone)]
        pub struct $name {
            $(
                pub $k: $t
            ),*
        }

        impl $crate::schema::Model for $name {}

        impl $crate::FromValue for $name {
            fn from_value(values: Vec<$crate::SqlValue>) -> Result<Self, $crate::Error>
            where
                Self: Sized
            {
                let mut values = values.into_iter();
                Ok(Self {
                    $(
                        $k: <$t as $crate::FromValue>::from_value(vec![values.next().expect("length mismatch. skill issue.")])?
                    ),*
                })
            }
        }

        pub const fn $name_def() -> $name_def::model {
            $name_def::model
        }

        #[allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
        mod besu_internal {
            use super::*;

            pub mod $name_def {
                use super::*;

                #[doc(hidden)]
                pub struct model;

                impl $crate::schema::Table for model {
                    type Model = super::$name;

                    fn def() -> $crate::schema::TableDefinition {
                        $crate::internal::construct::table_def(
                            stringify!($name_def),
                            &[$( stringify!($k) ),*],
                            &[$( stringify!($t) ),*]
                        )
                    }
                }

                $(
                    pub struct $k;

                    impl $crate::schema::Column for $k {
                        type Model = super::$name;
                        type Type = $t;

                        fn def() -> $crate::schema::ColumnDefinition {
                            $crate::internal::construct::column_def(stringify!($name_def), stringify!($k))
                        }
                    }
                )*

                pub use super::inner::{insert, insert_mut};
            }


            // The `pub const COLUMN_NAME` conflict so this separate them.
            mod inner {
                use super::*;

                #[derive(Debug, Default)] // TODO: Remove this `Default` impl once the macros are smarter
                pub struct insert {
                    $(
                        pub $ik: $it,
                    )*
                    $(
                        pub $iok: Option<$iot>
                    ),*
                }

                impl $crate::queries::InsertValue for insert {
                    fn def() -> $crate::queries::InsertDefinition {
                        $crate::internal::construct::insert_def(stringify!($name_def), &[$( stringify!($ik), )* $( stringify!($iok) ),*])
                    }

                    fn values(self) -> Vec<$crate::SqlValue> {
                        vec![
                            $(
                                self.$ik.into(),
                            )*
                            $(
                                self.$iok.into()
                            ),*
                        ]
                    }
                }

                impl insert {
                    pub fn edit(&mut self) -> insert_mut {
                        insert_mut(self)
                    }

                    $(
                        pub fn $ik<V: Into<$it>>(mut self, $ik: V) -> Self {
                            self.$ik = $ik.into();
                            self
                        }
                    )*

                    $(
                        pub fn $iok<V: Into<Option<$iot>>>(mut self, $iok: V) -> Self {
                            self.$iok = $iok.into();
                            self
                        }
                    )*
                }

                pub fn insert($($ik: impl Into<$it>),*) -> $name_def::insert {
                    $name_def::insert {
                        $(
                            $ik: $ik.into(),
                        )*
                        $(
                            $iok: None
                        ),*
                    }
                }

                pub struct insert_mut<'a>(&'a mut insert);

                impl<'a> insert_mut<'a> {
                    $(
                        pub fn $ik<V: Into<$it>>(&mut self, $ik: V) -> &mut Self {
                            self.0.$ik = $ik.into();
                            self
                        }
                    )*

                    $(
                        pub fn $iok<V: Into<Option<$iot>>>(&mut self, $iok: V) -> &mut Self {
                            self.0.$iok = $iok.into();
                            self
                        }
                    )*
                }
            }
        }

        pub use besu_internal::*;
    };

    (@internal; insert_ty; Option<$ty:path>) => {bool};
    (@internal; insert_ty; $ty:path) => {String};
}
