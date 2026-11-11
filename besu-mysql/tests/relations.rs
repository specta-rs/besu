//! Eager loading against a real MySQL server, mirroring `besu-sqlite/tests/relations.rs`.
//!
//! Every test gets its own database, like the rest of this crate's suite.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments
)]

use besu::prelude::*;
use besu_mysql::Sqlx;
use sqlx::MySqlPool;

/// The MySQL server the suite runs against.
const SERVER: &str = "mysql://root:besu@localhost:53306";

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "posts")]
pub struct Post {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub title: String,
    #[besu(references = "users.id", on_delete = "cascade")]
    pub author_id: i64,
}

/// A table whose key is text rather than an integer.
///
/// MySQL cannot index a `TEXT` column without a prefix length, so the key is a `VARCHAR`.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "teams")]
pub struct Team {
    #[besu(primary_key, sql_type = "besu::VarChar<64>")]
    pub slug: String,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "members")]
pub struct Member {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(
        sql_type = "besu::VarChar<64>",
        references = "teams.slug",
        on_delete = "cascade"
    )]
    pub team_slug: String,
    pub name: String,
}

/// Create an empty database named `name`, replacing any database left over from a previous
/// run.
async fn fresh(name: &str) -> besu::Database<Sqlx> {
    let admin = MySqlPool::connect(SERVER).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE `{name}`"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let url = format!("{SERVER}/{name}");
    besu::Database::new(Sqlx::new(&url).await.unwrap())
}

async fn setup(name: &str) -> besu::Database<Sqlx> {
    let db = fresh(name).await;
    db.push([users.def(), posts.def(), teams.def(), members.def()])
        .await
        .unwrap();
    db
}

/// Three users, posts for the first two, nothing for the third.
async fn seed(db: &besu::Database<Sqlx>) -> Vec<User> {
    for (email, name, age) in [
        ("alice@example.com", "Alice", 30),
        ("bob@example.com", "Bob", 20),
        ("carol@example.com", "Carol", 17),
    ] {
        db.insert(users)
            .value(users.insert(email, name, age))
            .await
            .unwrap();
    }

    // MySQL has no `RETURNING`, so the generated keys are read back.
    let parents: Vec<User> = db.select(users).order_by(users.id.asc()).await.unwrap();

    db.insert(posts)
        .value(posts.insert("Alice one", parents[0].id))
        .await
        .unwrap();
    db.insert(posts)
        .value(posts.insert("Alice two", parents[0].id))
        .await
        .unwrap();
    db.insert(posts)
        .value(posts.insert("Bob one", parents[1].id))
        .await
        .unwrap();

    parents
}

#[tokio::test]
async fn every_parent_gets_its_own_children() {
    let db = setup("besu_test_relations_every_parent").await;
    let seeded = seed(&db).await;

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id))
        .await
        .unwrap();

    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].0, seeded[0]);
    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Alice one", "Alice two"]
    );
    assert_eq!(
        loaded[1]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Bob one"]
    );

    // The whole point: a parent with no children is present with an empty `Vec`, not absent.
    assert_eq!(loaded[2].0.name, "Carol");
    assert!(loaded[2].1.is_empty());
}

#[tokio::test]
async fn generated_sql() {
    let db = besu::dry::<besu_mysql::MySql>();

    let (parent, child) = db
        .select(users)
        .filter(users.age.gt(18))
        .with(has_many(users.id, posts.author_id))
        .to_sql([3, 5, 9]);

    assert_eq!(
        parent.sql(),
        "SELECT `users`.`id`, `users`.`email`, `users`.`name`, `users`.`age`, `users`.`bio`, \
         `users`.`id` FROM `users` WHERE (`users`.`age` > ?)"
    );
    assert_eq!(parent.args().len(), 1);

    assert_eq!(
        child.sql(),
        "SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id`, `posts`.`author_id` \
         FROM `posts` WHERE (`posts`.`author_id` IN (?, ?, ?))"
    );
    assert_eq!(child.args().len(), 3);
}

#[tokio::test]
async fn no_second_query_when_the_parent_result_is_empty() {
    let db = setup("besu_test_relations_empty_parent").await;

    // If the relation load sent its child statement, this would fail rather than return
    // nothing. It must not: an empty parent result means no second round trip at all.
    db.execute(sql!("DROP TABLE `posts`")).await.unwrap();

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .with(has_many(users.id, posts.author_id))
        .await
        .unwrap();

    assert!(loaded.is_empty());
}

#[tokio::test]
async fn with_filter_order_and_pick() {
    let db = setup("besu_test_relations_pick").await;
    seed(&db).await;

    // `pick` replaces the projection and the row type; `with` appends the key behind it.
    let loaded: Vec<(String, Vec<Post>)> = db
        .select(users)
        .filter(users.age.gte(18))
        .order_by(users.name.desc())
        .pick(users.name)
        .with(has_many(users.id, posts.author_id))
        .await
        .unwrap();

    assert_eq!(
        loaded
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Bob", "Alice"]
    );
    assert_eq!(loaded[0].1.len(), 1);
    assert_eq!(loaded[1].1.len(), 2);
}

#[tokio::test]
async fn a_text_key_works_the_same_way() {
    let db = setup("besu_test_relations_text_key").await;

    for (slug, name) in [("rust", "Rust"), ("sql", "SQL")] {
        db.insert(teams)
            .value(teams.insert(slug, name))
            .await
            .unwrap();
    }
    db.insert(members)
        .value(members.insert("rust", "Alice"))
        .await
        .unwrap();
    db.insert(members)
        .value(members.insert("rust", "Bob"))
        .await
        .unwrap();

    let loaded: Vec<(Team, Vec<Member>)> = db
        .select(teams)
        .order_by(teams.slug.asc())
        .with(has_many(teams.slug, members.team_slug))
        .await
        .unwrap();

    assert_eq!(loaded.len(), 2);
    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>(),
        ["Alice", "Bob"]
    );
    assert!(loaded[1].1.is_empty());
}

#[tokio::test]
async fn loads_inside_a_transaction() {
    let db = setup("besu_test_relations_transaction").await;
    seed(&db).await;

    db.transaction(async |tx| {
        let loaded: Vec<(User, Vec<Post>)> = tx
            .select(users)
            .order_by(users.id.asc())
            .with(has_many(users.id, posts.author_id))
            .await?;

        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].1.len(), 2);
        assert!(loaded[2].1.is_empty());

        Ok(())
    })
    .await
    .unwrap();
}
