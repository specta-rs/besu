use std::path::Path;

use crate::queries::InsertQuery;

pub struct Database {}

impl Database {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {}
    }

    pub fn insert(&self) -> InsertQuery {
        InsertQuery {}
    }
}
