// TODO: Break this into it's own crate
// TODO: Fork sqlx w/ argument merging so we can cleanup the argument handling.

use std::borrow::Cow;

use sqlx::{mysql::MySqlArguments, Executor, MySqlPool};

use crate::{Argument, Arguments, Driver, Sql};

#[derive(Clone)]
pub struct MySql(MySqlPool);

impl MySql {
    pub fn new(pool: MySqlPool) -> crate::Db<Self> {
        crate::Db::new(Self(pool))
    }
}

// impl AsRef<MySqlPool> for crate::Db<MySql> {
//     fn as_ref(&self) -> &MySqlPool {
//         &self.0 .0
//     }
// }

impl Driver for MySql {
    type Error = sqlx::Error; // TODO: Seal this type or reexport it???
    type Arguments<'a> = MySqlArguments;

    fn escape_identifier<'a>(identifier: &'a str) -> Cow<'a, str> {
        // TODO: Check if already escape and bypass this?
        // TODO: Prevent escaping an identifier using backticks (`)
        Cow::Owned(format!("`{identifier}`"))
    }

    async fn execute(&self, sql: Sql<Arguments<'_, Self>>) -> Result<(), Self::Error> {
        let todo = (&*sql.sql(), sql.into_arguments());

        println!("EXECUTE: {todo:?}");

        self.0.execute(todo).await.map(|_| ())
    }
}

impl Argument<MySql> for String {
    fn insert<'q>(self, args: &mut Option<<MySql as Driver>::Arguments<'q>>) {
        sqlx::Arguments::add(args.get_or_insert_default(), self).unwrap();
    }
}

impl<'a> Argument<MySql> for &'a str {
    fn insert<'q>(self, args: &mut Option<<MySql as Driver>::Arguments<'q>>) {
        sqlx::Arguments::add(args.get_or_insert_default(), self).unwrap();
    }
}

impl Argument<MySql> for i32 {
    fn insert<'q>(self, args: &mut Option<<MySql as Driver>::Arguments<'q>>) {
        sqlx::Arguments::add(args.get_or_insert_default(), self).unwrap();
    }
}
