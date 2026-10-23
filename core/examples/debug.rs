#![allow(missing_docs)]

use besu_core::{Chunk, Dialect, Driver, Encode, Sql};

fn main() {
    println!("DEBUG");

    let db = Database(Sqlx);

    // TODO: this is the primitive of the `sql!` macro
    let name = "Monty".to_string();
    let p1 = <Sql<Sqlx>>::raw("SELECT * FROM users WHERE name = ");
    let p2 = Sql::<Sqlx, _>::param(name);
    let p3 = <Sql<Sqlx>>::raw(" ORDER BY id");
    let q = p1.append(p2).append(p3);
    db.exec(q);

    let name = "Monty".to_string();
    let p1 = Sql::raw("SELECT * FROM users WHERE name = ");
    let p2 = Sql::param(name);
    let p3 = Sql::raw(" ORDER BY id");
    let q = p1.append(p2).append(p3);
    db.exec(q);

    // let name = "Monty".to_string();
    // let q = Sql::join(
    //     (
    //         Sql::raw("SELECT * FROM users WHERE name = "),
    //         Sql::param(name),
    //         Sql::raw(" ORDER BY id"),
    //     ),
    //     Sql::default(),
    // );
    // db.exec(q);

    // TODO: Can we make this work???
    // let p2 = Sql::<Sqlx, _>::param(name);
    // let sql: Sql<Sqlx, _> = sql!("INSERT INTO {} {} {}", <Sql<Sqlx>>::raw(""), 42, ""); // TODO: What is SQL string vs literal string???

    // TODO: Should we have an `Sql` trait for other types that can be converted automatically???
}

pub struct Database<D: Driver>(D);

impl<D: Driver> Database<D> {
    pub fn exec<T>(&self, query: Sql<D, T>) {
        let (query, arguments) = D::Dialect::to_query(query);
        self.0.execute(query, arguments)
    }
}

pub struct Sqlite;

impl Dialect for Sqlite {
    fn to_query<D: Driver, T>(sql: Sql<D, T>) -> (String, Vec<D::Value>) {
        let mut query = String::new();
        let mut arguments = vec![];

        for chunk in sql.chunks {
            match chunk {
                Chunk::Raw(v) => query.push_str(&v),
                Chunk::Param(v) => {
                    query.push_str("?"); // TODO: Take this in as a config similar to `drizzle
                    arguments.push(v);
                }
            }
        }

        (query, arguments)
    }
}

#[derive(Debug)]
pub struct SqliteValue(String);

impl Encode<Sqlx> for String {
    fn encode(&self) -> <Sqlx as Driver>::Value {
        SqliteValue(format!("{self:?}"))
    }
}

pub struct Sqlx;
impl Driver for Sqlx {
    type Dialect = Sqlite;

    type Value = SqliteValue;

    fn execute(&self, sql: String, arguments: Vec<Self::Value>) {
        println!("{:?} {:?}", sql, arguments);
        // todo!()
    }
}
