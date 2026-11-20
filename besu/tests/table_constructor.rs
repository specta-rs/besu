//! Constructor bindings in derived tables.
#![cfg(feature = "derive")]
#![allow(missing_docs)]

use besu::{Insertable, Table, Value};

/// A tag with a required column that has the same name as the generated handle.
#[derive(Table)]
pub struct Tag {
    tag: String,
    position: i64,
}

#[test]
fn the_table_handle_does_not_shadow_a_required_constructor_argument() {
    assert_eq!(
        tag.insert("rust", 1i64).into_values(),
        [Some(Value::from("rust")), Some(Value::Int(1))]
    );
}
