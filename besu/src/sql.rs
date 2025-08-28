use std::borrow::Cow;

use crate::{into_sql, Argument, Driver, IntoSql, Join};

/// a placeholder for when an sql value is not possible.
///
/// TODO: Explain more
pub struct Never(());

pub struct Sql<T = Never> {
    arguments: T,
    parts: Parts,
}

enum Parts {
    Many(Vec<Part>),
    // Few([Option<Part>; 3]),
}

enum Part {
    Param,
    Identifier(Cow<'static, str>),
    Query(Cow<'static, str>),
}

pub struct Arguments<'a, D: Driver>(Option<D::Arguments<'a>>);

impl<T> Sql<T> {
    // pub fn map<F, U>(self, f: F) -> Sql<U>
    // where
    //     F: FnOnce(T) -> U,
    // {
    //     match self.0 {
    //         Parts::Many(parts) => Sql(Parts::Many(parts.into_iter().map(f).collect())),
    //         Parts::Few([a, b, c]) => Sql(Parts::Few([a.map(f), b.map(f), c.map(f)])),
    //     }
    // }

    // TODO
    // pub fn get_value(&self) -> &T {
    //     &self.arguments
    // }

    pub fn into_value(self) -> T {
        self.arguments
    }

    // TODO: With `MySql` this should infer to `'static`
    pub fn into_dialect<'a, D: Driver>(self) -> Sql<Arguments<'a, D>>
    where
        T: Argument<D>,
    {
        let mut args: Option<<D as Driver>::Arguments<'_>> = None;
        self.arguments.insert(&mut args);

        Sql {
            arguments: Arguments(args),
            parts: self.parts,
        }
    }
}

impl<'a, D: Driver> Sql<Arguments<'a, D>> {
    pub fn sql(&self) -> Cow<'static, str> {
        match &self.parts {
            Parts::Many(parts) => {
                let mut sql = String::new();
                for part in parts {
                    match part {
                        Part::Param => sql.push('?'),
                        Part::Identifier(ident) => sql.push_str(&D::escape_identifier(ident)),
                        Part::Query(query) => sql.push_str(&query),
                    }
                }
                Cow::Owned(sql)
            }
        }
    }

    pub fn arguments(&self) -> Option<&D::Arguments<'a>> {
        self.arguments.0.as_ref()
    }

    pub fn into_arguments(self) -> Option<D::Arguments<'a>> {
        self.arguments.0
    }

    // pub fn to_owned(&self) -> Sql<'static, D, T> {
    //     Sql {
    //         args: self.args.as_ref().map(D::to_owned_arguments),
    //         parts: self.parts.clone(),
    //         phantom: PhantomData,
    //     }
    // }
}

impl<D: Driver> Sql<Arguments<'static, D>> {
    // TODO: Allow downgrading lifetime?
}

// TODO: Debug, Clone, PartialEq, Eq, Ord, Hash, etc???

pub fn value<T>(value: T) -> Sql<T> {
    Sql {
        arguments: value,
        parts: Parts::Many(vec![Part::Param]),
        // parts: Parts::Few([Some(Part::Param(value)), None, None])
    }
}

pub fn sql(sql: &'static str) -> Sql<Never> {
    unsafe_sql(sql)
}

pub fn unsafe_sql(sql: impl Into<Cow<'static, str>>) -> Sql<Never> {
    Sql {
        arguments: Never(()),
        parts: Parts::Many(vec![Part::Query(sql.into())]),
        // parts: Parts::Few([Some(Part::Query(sql.into())), None, None]),
    }
}

pub fn identifier(identifier: impl Into<Cow<'static, str>>) -> Sql<Never> {
    Sql {
        arguments: Never(()),
        parts: Parts::Many(vec![Part::Identifier(identifier.into())]),
        // parts: Parts::Few([Some(Part::Identifier(identifier.into())), None, None]),
    }
}

pub fn null() -> Sql<Never> {
    sql("NULL")
}

// TODO: BREAK

impl<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11> Join
    for (
        Sql<T1>,
        Sql<T2>,
        Sql<T3>,
        Sql<T4>,
        Sql<T5>,
        Sql<T6>,
        Sql<T7>,
        Sql<T8>,
        Sql<T9>,
        Sql<T10>,
        Sql<T11>,
    )
{
    type Output = (T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);

    fn join(mut self) -> Sql<Self::Output> {
        let mut parts = Vec::default();
        parts.append(match &mut self.0.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.1.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.2.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.3.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.4.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.5.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.6.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.7.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.8.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.9.parts {
            Parts::Many(parts) => parts,
        });
        parts.append(match &mut self.10.parts {
            Parts::Many(parts) => parts,
        });

        Sql {
            arguments: (
                self.0.arguments,
                self.1.arguments,
                self.2.arguments,
                self.3.arguments,
                self.4.arguments,
                self.5.arguments,
                self.6.arguments,
                self.7.arguments,
                self.8.arguments,
                self.9.arguments,
                self.10.arguments,
            ),
            parts: Parts::Many(parts),
        }
    }
}

impl<T, U: From<T>> IntoSql<U, into_sql::FromSqlTToU> for Sql<T> {
    fn into_sql(self) -> Sql<U> {
        Sql {
            arguments: self.arguments.into(),
            parts: self.parts,
        }
    }
}

impl<T> IntoSql<T, into_sql::NeverIntoT> for Sql<Never> {
    fn into_sql(self) -> Sql<T> {
        Sql {
            arguments: todo!(),
            parts: self.parts,
        }
    }
}
