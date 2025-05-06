use std::{borrow::Cow, fmt, marker::PhantomData};

use crate::{Driver, Encode};

// TODO: Thoughts on giving this a lifetime???

// TODO: Make this an `enum` not a `struct`???
enum Chunk<D: Driver> {
    Raw(Cow<'static, str>),
    Param(D::Value),
}

pub struct Sql<D: Driver, T = ()> {
    chunks: Vec<Chunk<D>>, // TODO: Cow-style thing for fully static queries???
    phantom: PhantomData<T>,
}

// impl<D: Driver, T> fmt::Debug for Sql<D, T> {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         writeln!(f, "{:?}", self.chunks)
//     }
// }

impl<D: Driver, T> Sql<D, T> {
    pub fn raw(sql: impl Into<Cow<'static, str>>) -> Self {
        Self {
            chunks: vec![Chunk::Raw(sql.into())],
            phantom: PhantomData,
        }
    }

    pub fn param(value: T) -> Self
    where
        T: Encode<D>,
    {
        Self {
            chunks: vec![Chunk::Param(value.encode())],
            phantom: PhantomData,
        }
    }

    // pub fn identifier(name: impl Into<Cow<'static, str>>) -> Self {
    //     todo!();
    // }

    // pub fn placeholder() -> Self{
    //     todo!();
    // }

    // TODO: How to handle different `T`'s. We should probally allow a tuple here instead???
    // pub fn join(sql: Vec<Sql<D, T>>, separator: ()) {
    //     todo!();
    // }

    pub fn append<U>(mut self, mut sql: Sql<D, U>) -> Self {
        self.chunks.append(&mut sql.chunks);
        self
    }

    pub fn _as(self, alias: impl Into<Cow<'static, str>>) -> Sql<D, T> {
        todo!(); // TODO: How???
    }

    pub fn ty<U>(self) -> Sql<D, U> {
        todo!();
    }

    // TODO: Can we make this work??
    // pub fn map() {}

    // TODO: Can we make this work?
    // pub fn _if() {}

    // TODO: Maybe rename this
    pub fn to_query(self) -> (String, Vec<D::Value>) {
        let mut query = String::new();
        let mut arguments = vec![];

        for chunk in self.chunks {
            match chunk {
                Chunk::Raw(v) => query.push_str(&v),
                Chunk::Param(v) => arguments.push(v),
            }
        }

        (query, arguments)
    }
}
