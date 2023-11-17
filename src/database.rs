use std::{marker::PhantomData, path::Path};

use crate::queries::{InsertQuery, UpdateQuery};

pub struct Database {}

impl Database {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {}
    }

    pub fn insert(&self) -> InsertQuery {
        InsertQuery {}
    }

    // TODO: Generics
    pub fn update(&self, model: ()) -> UpdateQuery<()> {
        UpdateQuery {
            phantom: PhantomData,
        }
    }
}
