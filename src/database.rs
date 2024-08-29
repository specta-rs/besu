use std::{clone, fmt, sync::Arc};

use crate::{Dialect, Driver, Select, Model};

pub struct Database<D: Driver>(Arc<D>); // TODO: `Arc` or leave it to driver or 'a on queries???

impl<D: Driver> fmt::Debug for Database<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Database")
            .field(&D::NAME)
            .field(&D::Dialect::name())
            .finish()
    }
}

impl<D: Driver> clone::Clone for Database<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: Driver> Database<D> {
    /// Construct a new [`Database`] instance from a [`Driver`].
    pub fn new(driver: D) -> Self {
        Self(Arc::new(driver))
    }

    /// Get a reference to the [`Driver`] of the database.
    pub fn get_inner(&self) -> &D {
        &self.0
    }

    // /// Take the [`Driver`] out of the database instance.
    // pub fn into_inner(self) -> D {
    //     self.0
    // }

    pub fn select<T: Model>(&self, table: T) -> Select<D, T> {
        Select::new(self.0.clone())
    }

    // pub fn insert(&self) -> InsertQuery {
    //     InsertQuery { db: self.clone() }
    // }

    // pub fn update<T: Table>(&self, table: fn() -> T) -> UpdateQuery<T::Model> {
    //     UpdateQuery::new(self.clone(), table)
    // }

    // pub fn delete<T: Table>(&self, table: fn() -> T) -> DeleteQuery<T::Model> {
    //     DeleteQuery::new(self.clone(), table)
    // }
}
