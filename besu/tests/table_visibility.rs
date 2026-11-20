//! Derived tables keep the same visibility as their row model.
#![cfg(feature = "derive")]
#![allow(missing_docs, clippy::needless_pub_self)]
#![deny(unused_imports)]

use besu::{Insertable, Table, Value};

#[derive(Table)]
struct User {
    id: i64,
}

#[test]
fn a_private_model_can_be_the_tables_row_type() {
    let _: <UserTable as Table>::Model = User { id: 1 };
    let insert: UserInsert = user.insert(1i64);
    assert_eq!(insert.into_values(), [Some(Value::Int(1))]);
    assert_eq!(<UserTable as Table>::name(), "user");
}

mod schema {
    pub(crate) mod outer {
        pub(crate) mod inner {
            use besu::Table;

            #[derive(Table)]
            pub(self) struct Local {
                id: i64,
            }

            #[derive(Table)]
            pub(super) struct Parent {
                id: i64,
            }

            #[derive(Table)]
            pub(in super::super) struct Ancestor {
                id: i64,
            }

            #[derive(Table)]
            pub(in crate::schema) struct Absolute {
                id: i64,
            }

            #[derive(Table)]
            pub(crate) struct CrateWide {
                id: i64,
            }

            #[derive(Table)]
            pub struct Public {
                id: i64,
            }

            pub(crate) fn check_local() {
                let _: LocalInsert = local.insert(1i64);
            }
        }

        pub(crate) fn check_parent() {
            let _: inner::ParentInsert = inner::parent.insert(1i64);
        }
    }

    pub(crate) fn check_ancestor() {
        let _: outer::inner::AncestorInsert = outer::inner::ancestor.insert(1i64);
        let _: outer::inner::AbsoluteInsert = outer::inner::absolute.insert(1i64);
    }
}

pub use schema::outer::inner::{Public, PublicInsert, PublicTable, public};

#[test]
fn restricted_and_public_models_export_the_expected_names() {
    schema::outer::inner::check_local();
    schema::outer::check_parent();
    schema::check_ancestor();
    let _: schema::outer::inner::CrateWideInsert = schema::outer::inner::crate_wide.insert(1i64);
    let _: PublicInsert = public.insert(1i64);
    assert_eq!(<PublicTable as Table>::name(), "public");
}
