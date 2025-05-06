use std::{marker::PhantomData, pin::Pin};

use crate::Database;

// TODO: Break out
pub struct Select<'a, T> {
    pub(crate) db: &'a Database, // TODO: Lifetime or not?
    pub(crate) phantom: PhantomData<&'a T>,
}

impl<'a, T> Select<'a, T> {
    // TODO: Where
    // TODO: Ordering
    // TODO: Joins

    pub fn to_owned(&'a self) -> Select<'static, T> {
        todo!();
        // Select {
        //     db: self.db.clone(),
        //     phantom: PhantomData,
        // }
    }
}

impl<'a, T> IntoFuture for Select<'a, T> {
    type Output = Result<T, ()>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

    fn into_future(self) -> Self::IntoFuture {
        todo!()
    }
}
