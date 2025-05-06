use std::{fmt::Display, marker::PhantomData};

// TODO: Seal this
#[derive(Debug)]
pub enum Inner {
    Expr(String),
    ExprEscaped(String),
    Literal(&'static str),
}

// TODO: Seal this
#[derive(Debug)]
pub enum Inner2 {
    Owned(Vec<Inner>),
    TODO(Inner),
}

// TODO: Default type???
#[derive(Debug)]
pub struct Sql<T> {
    // TODO: Seal these fields
    pub parts: Inner2,
    pub marker: PhantomData<T>,
}
// TODO: Good debug impl + clone, etc traits.

impl<T> Sql<T> {
    pub const fn from_static_str(s: &'static str) -> Sql<T> {
        Self {
            parts: Inner2::TODO(Inner::Literal(s)),
            marker: PhantomData,
        }
    }

    pub fn dangerously_from_string(s: String) -> Sql<T> {
        Self {
            parts: Inner2::Owned(vec![Inner::Expr(s)]),
            marker: PhantomData,
        }
    }

    // pub fn ty<U>(self) -> Sql<U> {
    //     todo!();
    // }

    pub fn erase(self) -> Sql<()> {
        Sql {
            parts: self.parts,
            marker: PhantomData,
        }
    }
}

// TODO: Maybe deal with DB dialects???
impl<T> Display for Sql<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO
        // for part in &self.parts {
        //     match part {
        //         // TODO: Don't add a trailing space or when commas are approached (maybe we can read from input in groups of tokens to preserve some structure????)
        //         Inner::Expr(expr) => write!(f, "{} ", expr)?,
        //         Inner::ExprEscaped(expr) => write!(f, "'{}' ", expr)?, // TODO: Do SQL escaping properly
        //         Inner::Literal(lit) => write!(f, "{} ", lit)?,
        //     }
        // }
        Ok(())
    }
}

// TODO: `FromSql` bound good or not?
// impl<T: FromSql> IntoSql for Sql<T> {
//     type Type = T;

//     fn into_sql(self) -> Sql<Self::Type> {
//         self
//     }
// }
