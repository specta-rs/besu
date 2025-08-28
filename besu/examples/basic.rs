//! Basic example of Besu!

use besu::{identifier, null, sql, value, IntoSql, Join, MySql, Never, Sql};
use sqlx::MySqlPool;
// use besu_mysql::MySql;

#[tokio::main]
async fn main() {
    let db = besu::MySql::new(
        MySqlPool::connect(&"mysql://root:@localhost/besu")
            .await
            .unwrap(),
    );

    let one = value("todo").into_dialect::<MySql>();
    let two = value(42).into_dialect::<MySql>();
    let three = sql("SELECT 1;").into_dialect::<MySql>();

    db.execute(
        sql("INSERT INTO `groups`(id, name, notes, tenant) VALUES (UUID(), 'Oscar', 'Plain old SQL', 'default');")
    )
    .await
    .unwrap();

    db.execute(
        (
            sql("INSERT INTO "),
            identifier("groups"),
            sql("(id, name, notes, tenant) VALUES ("),
            sql("UUID()"),
            sql(", "),
            value("Oscar"),
            sql(", "),
            value("Besu primitive join"),
            sql(", "),
            value("default"),
            sql(");"),
        )
            .join(),
    )
    .await
    .unwrap();

    db.execute(Group::create(sql("UUID()"), "Oscar", "default").into_sql())
        .await
        .unwrap();
}

pub struct Group {
    id: Sql<String>,
    name: Sql<String>,
    // notes: Sql<Option<String>>,
    tenant: Sql<String>,
}

impl Group {
    pub fn create<M1, M2, M3>(
        id: impl IntoSql<String, M1>,
        name: impl IntoSql<String, M2>,
        tenant: impl IntoSql<String, M3>,
    ) -> Self {
        Self {
            id: id.into_sql(),
            name: name.into_sql(),
            tenant: tenant.into_sql(),
        }
    }

    // TODO: Fix return type. Can it be `Sql<Group>`
    pub fn into_sql(
        self,
    ) -> Sql<(
        Option<Never>,
        Option<Never>,
        Option<Never>,
        Option<String>,
        Option<Never>,
        Option<String>,
        Option<Never>,
        Option<String>,
        Option<Never>,
        Option<String>,
        Option<Never>,
    )> {
        (
            sql("INSERT INTO "),
            identifier("groups"),
            sql("(id, name, notes, tenant) VALUES ("),
            self.id,
            sql(", "),
            self.name,
            sql(", "),
            value("Group::create".into()),
            sql(", "),
            self.tenant,
            sql(");"),
        )
            .join()
    }
}

// TODO:
//  - Query joining
//  - Arguments working
//  - Identifiers working
//  - Zero-copy for SQLite + infer `'static` for MySQL
//  - Streaming responses
//  - Query responses
//  - Prepared queries
//  - Logging middleware

// TODO: `param`, `value` and `argument` are being used interchangeably
