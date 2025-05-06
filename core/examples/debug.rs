#![allow(missing_docs)]

use besu_core::{Driver, Encode, Sql};

fn main() {
    println!("DEBUG");

    let db = Database(Sqlx);

    let name = "Monty".to_string();
    let p1 = <Sql<Sqlx>>::raw("SELECT * FROM users WHERE name = ");
    let p2 = Sql::<Sqlx, _>::param(name);

    let q = p1.append(p2);

    db.exec(q);
}

pub struct Database<D: Driver>(D);

impl<D: Driver> Database<D> {
    pub fn exec<T>(&self, query: Sql<D, T>) {
        let (query, arguments) = query.to_query();

        self.0.execute(query, arguments)
    }
}

pub struct Sqlx;
impl Driver for Sqlx {
    type Value = SqliteValue;

    fn execute(&self, sql: String, arguments: Vec<Self::Value>) {
        println!("{:?} {:?}", sql, arguments);
        todo!()
    }
}

#[derive(Debug)]
pub struct SqliteValue(String);

impl Encode<Sqlx> for String {
    fn encode(&self) -> <Sqlx as Driver>::Value {
        SqliteValue(format!("{self:?}"))
    }
}
