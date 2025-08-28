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
        self.into_value().insert(args);
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
    > Argument<D> for (T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11)
{
    fn insert<'q>(self, args: &mut Option<<D as Driver>::Arguments<'q>>) {
        self.0.insert(args);
        self.1.insert(args);
        self.2.insert(args);
        self.3.insert(args);
        self.4.insert(args);
        self.5.insert(args);
        self.6.insert(args);
        self.7.insert(args);
        self.8.insert(args);
        self.9.insert(args);
        self.10.insert(args);
    }
}
