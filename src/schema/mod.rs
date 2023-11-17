mod field;
mod model;

#[macro_export]
macro_rules! table {
    ($name:ident, { $($k:ident: $t:ty),* $(,)? }) => {
        // #[allow(non_snake_case)]
        // pub const $name: $crate::Table = $crate::Table::new(stringify!($name), &[
        //     $(
        //        (stringify!($k), stringify!($t))
        //     ),*
        // ]);

        // #[allow(non_snake_case)]
        // pub mod $name {
        //     use super::*;
        //     $(
        //         pub const $k: $crate::Column<$t> = $crate::Column::new();
        //     )*

        //     mod inner {
        //         // TODO: Handle required vs non-required fields -> Can fields define a default value?
        //         pub fn values($($k: $t),*) {
        //             todo!();
        //         }
        //     }

        //     pub use inner::*;

        //     // pub(super) struct InsertStruct {
        //     //     $(
        //     //         pub $k: $t,
        //     //     )*
        //     // }
        // }
    };
}
