mod column;
mod table;

pub use column::Column;
pub use table::Table;

// TODO: Error on reserved field names - `insert`, etc

#[macro_export]
macro_rules! table {
    ($name:ident, { $($k:ident: $t:ty $(; $(@default($default:expr))? )?),* $(,)? }, {
        insert: { $($ik:ident: $it:ty),* $(,)? },
    }) => {
        #[allow(non_upper_case_globals)]
        pub const $name: $crate::schema::Table = $crate::internal::construct::table(stringify!($name), &[
            $(
               (stringify!($k), stringify!($t))
            ),*
        ]);

        #[allow(non_snake_case, non_upper_case_globals)]
        pub mod $name {
            $(
                pub const $k: $crate::schema::Column<$t> = $crate::internal::construct::column(stringify!($k));
            )*

            // The `pub const COLUMN_NAME` conflict so this separate them.
            mod inner {
                #[allow(non_camel_case_types)]
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

                #[allow(non_camel_case_types)]
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
