//! Eager loading against a real MySQL server, mirroring `besu-sqlite/tests/relations.rs`.
//!
//! Every test gets its own database, like the rest of this crate's suite.
#![cfg(feature = "sqlx")]
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

use besu::prelude::*;
use besu::types::Int8;
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

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "comments")]
pub struct Comment {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub body: String,
    #[besu(references = "posts.id", on_delete = "cascade")]
    pub post_id: i64,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "tags")]
pub struct Tag {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub name: String,
}

#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "post_tags")]
pub struct PostTag {
    #[besu(primary_key)]
    pub post_id: i64,
    #[besu(primary_key)]
    pub tag_id: i64,
}

/// A relation declared once and reused.
const TAGS: ManyToMany<PostTable, PostTagTable, TagTable, Int8, Int8> =
    many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id);

async fn setup(name: &str) -> besu::Database<Sqlx> {
    let db = fresh(name).await;
    db.push([
        users.def(),
        posts.def(),
        comments.def(),
        tags.def(),
        post_tags.def(),
        teams.def(),
        members.def(),
    ])
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

    let statements = db
        .select(users)
        .filter(users.age.gt(18))
        .with(has_many(users.id, posts.author_id))
        .to_sql([3, 5, 3]);

    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[0].sql(),
        "SELECT `users`.`id`, `users`.`email`, `users`.`name`, `users`.`age`, `users`.`bio`, \
         `users`.`id` FROM `users` WHERE (`users`.`age` > ?)"
    );
    assert_eq!(statements[0].args().len(), 1);

    assert_eq!(
        statements[1].sql(),
        "SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id`, `posts`.`author_id` \
         FROM `posts` WHERE (`posts`.`author_id` IN (?, ?))"
    );
    assert_eq!(statements[1].args().len(), 2);
}

#[tokio::test]
async fn no_second_query_when_the_parent_result_is_empty() {
    let db = setup("besu_test_relations_empty_parent").await;

    // If the relation load sent its child statement, this would fail rather than return
    // nothing. It must not: an empty parent result means no second round trip at all.
    db.execute(sql!("DROP TABLE `comments`")).await.unwrap();
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

/// A const relation value loads many-to-many children in three statements, preserving parent
/// order and giving a childless parent an empty `Vec`.
#[tokio::test]
async fn many_to_many_loads_children_for_every_parent() {
    let db = setup("besu_test_relations_m2m").await;
    let seeded = seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for name in ["rust", "sql", "orm"] {
        db.insert(tags).value(tags.insert(name)).await.unwrap();
    }
    let created: Vec<Tag> = db.select(tags).order_by(tags.id.asc()).await.unwrap();
    for (post, tag) in [
        (posts_by_id[0].id, created[0].id),
        (posts_by_id[0].id, created[1].id),
        (posts_by_id[1].id, created[0].id),
        (posts_by_id[2].id, created[2].id),
    ] {
        db.insert(post_tags)
            .value(post_tags.insert(post, tag))
            .await
            .unwrap();
    }
    db.insert(posts)
        .value(posts.insert("Bob two", seeded[1].id))
        .await
        .unwrap();

    let loaded: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .order_by(posts.id.asc())
        .with(TAGS)
        .await
        .unwrap();

    assert_eq!(loaded.len(), 4);
    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["rust", "sql"]
    );
    assert_eq!(
        loaded[1]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["rust"]
    );
    assert_eq!(
        loaded[2]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["orm"]
    );
    assert!(loaded[3].1.is_empty());
}

/// `users -> posts -> comments` is one statement per level, nested as the relation values
/// are, with empty vectors at every level that matched nothing.
#[tokio::test]
async fn nested_relations_load_one_statement_per_level() {
    let db = setup("besu_test_relations_nested").await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for (body, post) in [
        ("Alice one first", posts_by_id[0].id),
        ("Alice one second", posts_by_id[0].id),
        ("Bob one first", posts_by_id[2].id),
    ] {
        db.insert(comments)
            .value(comments.insert(body, post))
            .await
            .unwrap();
    }

    let loaded: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
        .await
        .unwrap();

    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].1.len(), 2);
    assert_eq!(loaded[0].1[0].1.len(), 2);
    assert!(loaded[0].1[1].1.is_empty());
    assert_eq!(loaded[1].1.len(), 1);
    assert_eq!(loaded[1].1[0].1.len(), 1);
    assert!(loaded[2].1.is_empty());
}

/// A level with no rows above it sends no statement, proved by dropping its table.
#[tokio::test]
async fn a_level_with_nothing_above_it_sends_no_statement() {
    let db = setup("besu_test_relations_level_skip").await;
    seed(&db).await;

    db.execute(sql!("DELETE FROM `posts`")).await.unwrap();
    db.execute(sql!("DROP TABLE `comments`")).await.unwrap();

    let loaded: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
        .await
        .unwrap();

    assert_eq!(loaded.len(), 3);
    assert!(
        loaded
            .iter()
            .all(|(_, posts_by_parent)| posts_by_parent.is_empty())
    );
}

/// The parent query keeps its filters, ordering, limit and projection with a relation
/// attached, and a many-to-many can nest under a has-many.
#[tokio::test]
async fn filters_order_limit_and_pick_still_apply_to_the_parent() {
    let db = setup("besu_test_relations_parent").await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    db.insert(tags).value(tags.insert("rust")).await.unwrap();
    let tag: Tag = db.select(tags).first().await.unwrap().unwrap();
    for post in &posts_by_id[..2] {
        db.insert(post_tags)
            .value(post_tags.insert(post.id, tag.id))
            .await
            .unwrap();
    }

    let picked: Vec<(String, Vec<Tag>)> = db
        .select(posts)
        .filter(posts.author_id.gt(0))
        .order_by(posts.id.desc())
        .limit(2)
        .pick(posts.title)
        .with(TAGS)
        .await
        .unwrap();
    assert_eq!(
        picked
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        ["Bob one", "Alice two"]
    );
    assert!(picked[0].1.is_empty());
    assert_eq!(picked[1].1.len(), 1);

    let nested: Vec<(User, Vec<(Post, Vec<Tag>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id).with(TAGS))
        .await
        .unwrap();

    assert_eq!(nested[0].1.len(), 2);
    assert_eq!(nested[0].1[0].1, vec![tag.clone()]);
    assert_eq!(nested[0].1[1].1, vec![tag]);
}

/// The limit is per parent, and the order moves within each parent rather than across the
/// whole result: Alice gets her newest post, Bob his only one, Carol still gets an empty
/// `Vec`.
#[tokio::test]
async fn per_parent_limit_and_order_on_has_many() {
    let db = setup("besu_test_relations_limit_has_many").await;
    seed(&db).await;

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.desc())
                .limit(1),
        )
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Alice two"]
    );
    assert_eq!(
        loaded[1]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Bob one"]
    );
    assert!(loaded[2].1.is_empty());
}

/// A limit larger than the number of children keeps all of them, in the requested order.
#[tokio::test]
async fn a_limit_larger_than_the_child_count_keeps_every_child() {
    let db = setup("besu_test_relations_limit_large").await;
    seed(&db).await;

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.title.asc())
                .limit(50),
        )
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Alice one", "Alice two"]
    );
    assert_eq!(loaded[1].1.len(), 1);
    assert!(loaded[2].1.is_empty());
}

/// With no order, a limit falls back to the child's primary key rather than returning an
/// arbitrary row.
#[tokio::test]
async fn a_limit_without_an_order_uses_the_child_key() {
    let db = setup("besu_test_relations_limit_fallback").await;
    seed(&db).await;

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id).limit(1))
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|post| post.title.as_str())
            .collect::<Vec<_>>(),
        ["Alice one"]
    );
    assert_eq!(loaded[1].1[0].title, "Bob one");
}

/// `pick` replaces the parent row type; the limit still applies to the children and the
/// parent still gets an entry, even with none.
#[tokio::test]
async fn a_limit_works_with_a_parent_pick() {
    let db = setup("besu_test_relations_limit_pick").await;
    seed(&db).await;

    let loaded: Vec<(String, Vec<Post>)> = db
        .select(users)
        .order_by(users.name.asc())
        .pick(users.name)
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.desc())
                .limit(1),
        )
        .await
        .unwrap();

    assert_eq!(
        loaded
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Alice", "Bob", "Carol"]
    );
    assert_eq!(loaded[0].1.len(), 1);
    assert_eq!(loaded[0].1[0].title, "Alice two");
    assert_eq!(loaded[1].1.len(), 1);
    assert!(loaded[2].1.is_empty());
}

/// Many-to-many children are ranked per parent after the join, so each post gets its own
/// top tag and a post with none keeps an empty `Vec`.
#[tokio::test]
async fn per_parent_limit_and_order_on_many_to_many() {
    let db = setup("besu_test_relations_limit_m2m").await;
    let seeded = seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for name in ["rust", "sql", "orm"] {
        db.insert(tags).value(tags.insert(name)).await.unwrap();
    }
    let created: Vec<Tag> = db.select(tags).order_by(tags.id.asc()).await.unwrap();
    for (post, tag) in [
        (posts_by_id[0].id, created[0].id),
        (posts_by_id[0].id, created[1].id),
        (posts_by_id[1].id, created[0].id),
        (posts_by_id[2].id, created[2].id),
    ] {
        db.insert(post_tags)
            .value(post_tags.insert(post, tag))
            .await
            .unwrap();
    }
    db.insert(posts)
        .value(posts.insert("Bob two", seeded[1].id))
        .await
        .unwrap();

    let limited: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .order_by(posts.id.asc())
        .with(TAGS.order_by(tags.name.desc()).limit(1))
        .await
        .unwrap();

    assert_eq!(
        limited[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["sql"]
    );
    assert_eq!(
        limited[1]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["rust"]
    );
    assert_eq!(
        limited[2]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["orm"]
    );
    assert!(limited[3].1.is_empty());

    let all: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .order_by(posts.id.asc())
        .with(TAGS.order_by(tags.name.desc()).limit(10))
        .await
        .unwrap();
    assert_eq!(
        all[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["sql", "rust"]
    );
}

/// Each level of a nested relation carries its own limit and order.
#[tokio::test]
async fn per_parent_limits_apply_at_each_nested_level() {
    let db = setup("besu_test_relations_limit_nested").await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for (body, post) in [
        ("Alice one first", posts_by_id[0].id),
        ("Alice one second", posts_by_id[0].id),
        ("Bob one first", posts_by_id[2].id),
    ] {
        db.insert(comments)
            .value(comments.insert(body, post))
            .await
            .unwrap();
    }

    let loaded: Vec<(User, Vec<(Post, Vec<Comment>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.asc())
                .limit(2)
                .with(
                    has_many(posts.id, comments.post_id)
                        .order_by(comments.id.desc())
                        .limit(1),
                ),
        )
        .await
        .unwrap();

    assert_eq!(loaded[0].1.len(), 2);
    assert_eq!(
        loaded[0].1[0]
            .1
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["Alice one second"]
    );
    assert!(loaded[0].1[1].1.is_empty());
    assert_eq!(loaded[1].1.len(), 1);
    assert_eq!(loaded[1].1[0].1[0].body, "Bob one first");
    assert!(loaded[2].1.is_empty());
}

/// The exact SQL a real MySQL load runs: one statement with a windowed derived table.
#[tokio::test]
async fn generated_sql_for_a_per_parent_limit() {
    let db = besu::dry::<besu_mysql::MySql>();

    let statements = db
        .select(users)
        .filter(users.age.gt(18))
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.title.desc())
                .limit(3),
        )
        .to_sql([3, 5]);

    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[1].sql(),
        concat!(
            "SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id`, `posts`.`author_id` ",
            "FROM (SELECT `posts`.`id`, `posts`.`title`, `posts`.`author_id`, ",
            "ROW_NUMBER() OVER (PARTITION BY `posts`.`author_id` ORDER BY `posts`.`title` DESC) ",
            "AS `besu_row` FROM `posts` WHERE (`posts`.`author_id` IN (?, ?))) AS `posts` ",
            "WHERE (`posts`.`besu_row` <= ?) ORDER BY `posts`.`besu_row` ASC"
        )
    );
}

/// A limited many-to-many can sit under a limited has-many: each level ranks independently.
#[tokio::test]
async fn a_limited_many_to_many_nests_under_a_limited_has_many() {
    let db = setup("besu_test_relations_limit_nested_m2m").await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for name in ["rust", "sql", "orm"] {
        db.insert(tags).value(tags.insert(name)).await.unwrap();
    }
    let created: Vec<Tag> = db.select(tags).order_by(tags.id.asc()).await.unwrap();
    for (post, tag) in [
        (posts_by_id[0].id, created[0].id),
        (posts_by_id[0].id, created[1].id),
        (posts_by_id[1].id, created[0].id),
        (posts_by_id[2].id, created[2].id),
    ] {
        db.insert(post_tags)
            .value(post_tags.insert(post, tag))
            .await
            .unwrap();
    }

    let loaded: Vec<(User, Vec<(Post, Vec<Tag>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(
            has_many(users.id, posts.author_id)
                .order_by(posts.id.asc())
                .limit(2)
                .with(TAGS.order_by(tags.name.desc()).limit(1)),
        )
        .await
        .unwrap();

    assert_eq!(loaded[0].1.len(), 2);
    assert_eq!(
        loaded[0].1[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["sql"]
    );
    assert_eq!(loaded[0].1[1].1[0].name, "rust");
    assert_eq!(loaded[1].1[0].1[0].name, "orm");
    assert!(loaded[2].1.is_empty());
}

/// Ordering a many-to-many without a limit still uses the ranked statement, because the
/// join rows do not carry the child order.
#[tokio::test]
async fn many_to_many_order_without_a_limit_orders_children() {
    let db = setup("besu_test_relations_m2m_order").await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    for name in ["rust", "sql", "orm"] {
        db.insert(tags).value(tags.insert(name)).await.unwrap();
    }
    let created: Vec<Tag> = db.select(tags).order_by(tags.id.asc()).await.unwrap();
    for (post, tag) in [
        (posts_by_id[0].id, created[0].id),
        (posts_by_id[0].id, created[1].id),
        (posts_by_id[2].id, created[2].id),
    ] {
        db.insert(post_tags)
            .value(post_tags.insert(post, tag))
            .await
            .unwrap();
    }

    let loaded: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .order_by(posts.id.asc())
        .with(TAGS.order_by(tags.name.asc()))
        .await
        .unwrap();

    assert_eq!(
        loaded[0]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["rust", "sql"]
    );
    assert!(loaded[1].1.is_empty());
    assert_eq!(
        loaded[2]
            .1
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["orm"]
    );
}
