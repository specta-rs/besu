mod column;
mod table;

pub use column::Column;
pub use table::{Table, TableDef};

// TODO: Error on reserved field names - `insert`, etc

#[macro_export]
macro_rules! table {
    ($name:ident, $name_def:ident, { $($k:ident: $t:ty $(; $(@default($default:expr))? )?),* $(,)? }, {
        insert: { $($ik:ident: $it:ty),* $(,)? },
    }) => {
        #[allow(non_upper_case_globals)]
        pub const $name_def: $name_def::Table = $name_def::new_table();

        #[derive(Debug)]
        pub struct $name {
            $(
                pub $k: $t
            ),*
        }

        #[allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
        pub mod $name_def {
            $(
                pub const $k: $crate::schema::Column<$t> = $crate::internal::construct::column(stringify!($k));
            )*


            pub struct Table(());

            #[doc(hidden)]
            pub(super) const fn new_table() -> Table {
                Table(())
            }

            impl $crate::schema::Table for Table {
                fn def() -> $crate::schema::TableDef {
                    $crate::internal::construct::table_def(stringify!($name), &[
                        $(
                            (stringify!($k), stringify!($t))
                        ),*
                    ])
                }
            }

            // The `pub const COLUMN_NAME` conflict so this separate them.
            mod inner {
                #[derive(Debug)]
                pub struct insert {
                    $(
                        pub $ik: $it
                    ),*
                }

                impl $crate::queries::InsertValue for insert {}

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
                }

                pub struct insert_mut<'a>(&'a mut insert);

                impl<'a> insert_mut<'a> {
                    $(
                        pub fn $ik<V: Into<$it>>(&mut self, $ik: V) {
                            self.0.$ik = $ik.into();
                        }
                    )*
                }

                pub fn insert($($ik: impl Into<$it>),*) -> insert {
                    insert {
                        $(
                            $ik: $ik.into()
                        ),*
                    }
                }
            }

            pub use inner::{insert};
        }
    };

    (@internal; insert_ty; Option<$ty:path>) => {bool};
    (@internal; insert_ty; $ty:path) => {String};
}
