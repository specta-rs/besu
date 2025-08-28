use std::borrow::Cow;

use crate::{Arguments, Never, Sql};

/// A trait representing a database driver.
///
/// This trait defines the operations required for executing an [Sql] statement against a specific database.
///
/// ## Drivers
///
/// - [besu-mysql](https://docs.rs/besu-mysql) for [MySQL](https://www.mysql.com) support.
/// - [besu-sqlite](https://docs.rs/besu-sqlite) for [SQLite](https://www.sqlite.org) support.
/// - [besu-postgres](https://docs.rs/besu-postgres) for [PostgreSQL](https://www.postgresql.org) support.
///
pub trait Driver: Sized {
    type Error: std::error::Error;
    type Arguments<'a>;

    fn escape_identifier<'a>(identifier: &'a str) -> Cow<'a, str>;

    fn execute(
        &self,
        sql: Sql<Arguments<'_, Self>>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>>;
}

// TODO: This trait can be implemented in a way to break all internal expectations on `Parts`. Can we fix that?
pub trait Argument<D: Driver> {
    fn insert<'q>(self, args: &mut Option<D::Arguments<'q>>);
}

impl<D: Driver, T: Argument<D>> Argument<D> for Sql<T> {
    fn insert<'q>(self, args: &mut Option<<D as Driver>::Arguments<'q>>) {
        // TODO: Is this sane?
        if let Some(value) = self.into_value() {
            value.insert(args);
        }
    }
}

impl<D: Driver> Argument<D> for Never {
    fn insert<'q>(self, _: &mut Option<D::Arguments<'q>>) {}
}

// TODO: Implement all of them
impl<
        D: Driver,
        T1: Argument<D>,
        T2: Argument<D>,
        T3: Argument<D>,
        T4: Argument<D>,
        T5: Argument<D>,
        T6: Argument<D>,
        T7: Argument<D>,
        T8: Argument<D>,
        T9: Argument<D>,
        T10: Argument<D>,
        T11: Argument<D>,
    > Argument<D>
    for (
        // TODO: Wrapping in option probs breaks composability cause what if arg is option???
        Option<T1>,
        Option<T2>,
        Option<T3>,
        Option<T4>,
        Option<T5>,
        Option<T6>,
        Option<T7>,
        Option<T8>,
        Option<T9>,
        Option<T10>,
        Option<T11>,
    )
{
    fn insert<'q>(self, args: &mut Option<<D as Driver>::Arguments<'q>>) {
        if let Some(value) = self.0 {
            value.insert(args);
        }
        if let Some(value) = self.1 {
            value.insert(args);
        }
        if let Some(value) = self.2 {
            value.insert(args);
        }
        if let Some(value) = self.3 {
            value.insert(args);
        }
        if let Some(value) = self.4 {
            value.insert(args);
        }
        if let Some(value) = self.5 {
            value.insert(args);
        }
        if let Some(value) = self.6 {
            value.insert(args);
        }
        if let Some(value) = self.7 {
            value.insert(args);
        }
        if let Some(value) = self.8 {
            value.insert(args);
        }
        if let Some(value) = self.9 {
            value.insert(args);
        }
        if let Some(value) = self.10 {
            value.insert(args);
        }
    }
}
