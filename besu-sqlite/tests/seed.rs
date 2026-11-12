//! Deterministic seeding against a real SQLite database.
//!
//! The point of these tests is the guarantees, not the ergonomics: the same seed produces the
//! same rows, a unique column does not collide, and a foreign key always points at a row that
//! was actually inserted.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::collections::HashSet;

use besu::prelude::*;
use besu::seed::seed;
use besu_sqlite::Sqlx;

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(unique)]
    pub email: String,
    #[besu(unique)]
    pub external_id: i64,
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
    pub published: bool,
    pub score: f64,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "teams")]
pub struct Team {
    #[besu(primary_key)]
    pub slug: String,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "members")]
pub struct Member {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    #[besu(references = "teams.slug", on_delete = "cascade")]
    pub team_slug: String,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "line_items")]
pub struct LineItem {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub price: i64,
    pub quantity: i64,
    #[besu(generated = "price * quantity")]
    pub total: i64,
}

async fn setup() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([
        users.def(),
        posts.def(),
        teams.def(),
        members.def(),
        line_items.def(),
    ])
    .await
    .unwrap();
    db
}

#[tokio::test]
async fn a_thousand_rows_land_with_unique_emails_and_real_parents() {
    let db = setup().await;

    let report = seed(&db)
        .seed(42)
        .table(users, 1_000)
        .table(posts, 1_000)
        .await
        .unwrap();

    assert_eq!(report.rows("users"), 1_000);
    assert_eq!(report.rows("posts"), 1_000);
    assert_eq!(report.total(), 2_000);
    assert_eq!(report.tables().count(), 2);

    let people: Vec<User> = db.select(users).order_by(users.id.asc()).await.unwrap();
    assert_eq!(people.len(), 1_000);

    // Auto-incrementing keys are written explicitly from 1, which is what makes the parent
    // pool available without `RETURNING`.
    assert_eq!(people.first().unwrap().id, 1);
    assert_eq!(people.last().unwrap().id, 1_000);

    let emails: HashSet<&str> = people.iter().map(|user| user.email.as_str()).collect();
    assert_eq!(emails.len(), 1_000, "the unique text column collided");
    assert!(people.iter().all(|user| user.email.contains('@')));

    let external_ids: HashSet<i64> = people.iter().map(|user| user.external_id).collect();
    assert_eq!(
        external_ids.len(),
        1_000,
        "the unique integer column collided"
    );

    // A nullable column must exercise both states, and a NOT NULL one must never be absent.
    assert!(people.iter().any(|user| user.bio.is_none()));
    assert!(people.iter().any(|user| user.bio.is_some()));
    assert!(people.iter().all(|user| !user.name.is_empty()));

    let stored_posts: Vec<Post> = db.select(posts).await.unwrap();
    assert_eq!(stored_posts.len(), 1_000);

    let user_ids: HashSet<i64> = people.iter().map(|user| user.id).collect();
    assert!(
        stored_posts
            .iter()
            .all(|post| user_ids.contains(&post.author_id)),
        "a foreign key points at a row that does not exist"
    );

    // Every key came from a real parent, and the pool covers more than one row: a constant
    // foreign key would make the join tests meaningless.
    let authors: HashSet<i64> = stored_posts.iter().map(|post| post.author_id).collect();
    assert!(authors.len() > 100, "the key pool was not used");
}

#[tokio::test]
async fn the_same_seed_produces_the_same_rows() {
    let first = setup().await;
    let second = setup().await;

    seed(&first)
        .seed(7)
        .table(users, 50)
        .table(posts, 50)
        .await
        .unwrap();
    seed(&second)
        .seed(7)
        .table(users, 50)
        .table(posts, 50)
        .await
        .unwrap();

    let first_users: Vec<User> = first.select(users).order_by(users.id.asc()).await.unwrap();
    let second_users: Vec<User> = second.select(users).order_by(users.id.asc()).await.unwrap();
    assert_eq!(first_users, second_users);

    // Foreign keys are drawn from a parent pool by row, so they have to replay too.
    let first_posts: Vec<Post> = first.select(posts).order_by(posts.id.asc()).await.unwrap();
    let second_posts: Vec<Post> = second.select(posts).order_by(posts.id.asc()).await.unwrap();
    assert_eq!(first_posts, second_posts);
}

#[tokio::test]
async fn a_text_primary_key_can_be_a_parent() {
    let db = setup().await;

    let report = seed(&db)
        .table(teams, 20)
        .table(members, 100)
        .await
        .unwrap();
    assert_eq!(report.rows("teams"), 20);
    assert_eq!(report.rows("members"), 100);

    let stored_teams: Vec<Team> = db.select(teams).await.unwrap();
    let slugs: HashSet<&str> = stored_teams.iter().map(|team| team.slug.as_str()).collect();
    assert_eq!(slugs.len(), 20, "the text primary key collided");

    let stored_members: Vec<Member> = db.select(members).await.unwrap();
    assert_eq!(stored_members.len(), 100);
    assert!(
        stored_members
            .iter()
            .all(|member| slugs.contains(member.team_slug.as_str())),
        "a text foreign key points at a team that does not exist"
    );
}

#[tokio::test]
async fn an_override_replaces_the_generated_value() {
    let db = setup().await;

    seed(&db)
        .table(users, 1)
        .set(users.email, "fixed@example.com")
        .set(users.name, "Oscar")
        .set(users.bio, None::<String>)
        .await
        .unwrap();

    let stored: Vec<User> = db.select(users).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].email, "fixed@example.com");
    assert_eq!(stored[0].name, "Oscar");
    assert_eq!(stored[0].bio, None);
}

#[tokio::test]
async fn a_generated_column_is_left_to_the_database() {
    let db = setup().await;

    let report = seed(&db).table(line_items, 20).await.unwrap();
    assert_eq!(report.rows("line_items"), 20);

    let stored: Vec<LineItem> = db.select(line_items).await.unwrap();
    assert_eq!(stored.len(), 20);
    assert!(
        stored
            .iter()
            .all(|item| item.total == item.price * item.quantity),
        "the generated column was written by the seed"
    );
}

#[tokio::test]
async fn a_child_without_its_parent_is_an_error() {
    let db = setup().await;

    // No `users` rows exist, so there is no key to point at. Inventing one would produce a
    // dangling foreign key, which is exactly what this API refuses to do.
    let error = seed(&db).table(posts, 5).await.unwrap_err();
    let message = error.to_string();
    assert!(message.contains("users.id"), "{message}");
    assert!(message.contains("seed `users` first"), "{message}");

    let stored_posts: Vec<Post> = db.select(posts).await.unwrap();
    assert!(
        stored_posts.is_empty(),
        "a failed seed must not insert anything"
    );
}
