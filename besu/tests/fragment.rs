//! Driverless tests for composing raw SQL fragments.
#![allow(clippy::unwrap_used)]

use std::borrow::Cow;

use besu::{Dialect, Error, Fragment, Sql, TypeDef, Value};
use besu_core::{Placeholder, UpsertStyle};

struct Numbered;

impl Dialect for Numbered {
    type Value = Value;
    const NAME: &'static str = "numbered";
    const PLACEHOLDER: Placeholder = Placeholder::Dollar;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = true;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    fn bind(value: Value) -> Result<Self::Value, Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        out.push_str(&format!("${}", index + 1));
    }

    fn type_name(_ty: TypeDef) -> Cow<'static, str> {
        Cow::Borrowed("TEXT")
    }
}

#[test]
fn a_fragment_keeps_the_error_from_a_rendered_statement() {
    let error = Error::invalid_query("test build error");
    let mut invalid = Sql::<Numbered>::raw("SELECT 1");
    invalid.set_error(error.clone());

    let mut fragment = Fragment::from(Sql::<Numbered>::raw("EXPLAIN "));
    fragment.push(Fragment::from(invalid));
    assert_eq!(fragment.render::<Numbered>().finish().unwrap_err(), error);
}
