use std::{any::type_name, fmt};

use crate::{Arguments, Delete, Driver, Execute, Insert, Select, Table, Update};

#[derive(Clone)]
pub struct Database<D: Driver>(D);

impl<D: Driver> fmt::Debug for Database<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Database")
            .field(&type_name::<D>())
            .field(&type_name::<D::Dialect>())
            .finish()
    }
}

impl<D: Driver> Database<D> {
    /// Construct a new [`Database`] instance from a [`Driver`].
    pub fn new(driver: D) -> Self {
        Self(driver)
    }

    /// Get a reference to the [`Driver`] of the database.
    pub fn driver(&self) -> &D {
        &self.0
    }

    /// Construct a new [`Select`] operation for the given table.
    pub fn select<T: Table>(&self, table: T) -> Select<D, T> {
        let _ = table;
        Select::new(self.0.clone())
    }

    /// Construct a new [`Insert`] operation for the given table.
    pub fn insert<T: Table>(&self, table: T) -> Insert<D, T> {
        let _ = table;
        Insert::new(self.0.clone())
    }

    /// Construct a new [`Update`] operation for the given table.
    pub fn update<T: Table>(&self, table: T) -> Update<D, T> {
        let _ = table;
        Update::new(self.0.clone())
    }

    /// Construct a new [`Delete`] operation for the given table.
    pub fn delete<T: Table>(&self, table: T) -> Delete<D, T> {
        let _ = table;
        Delete::new(self.0.clone())
    }

    /// Construct a new [`Execute`] operation.
    pub fn execute<'a, T: Arguments<'a, D>>(
        &self,
        (sql, arguments): (&'static str, T),
    ) -> Execute<'a, D> {
        Execute::new(self.0.clone(), sql, arguments.into_driver_arguments())
    }

    /// Construct a new [`Execute`] operation.
    ///
    /// This method takes a non-static string so it is potentially prone to SQL injection so take care!
    pub fn execute_unsafe<'a, T: Arguments<'a, D>>(
        &self,
        (sql, arguments): (&'a str, T),
    ) -> Execute<'a, D> {
        Execute::new(self.0.clone(), sql, arguments.into_driver_arguments())
    }
}
