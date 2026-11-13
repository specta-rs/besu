#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

//! Eager loading against SQLite: one batched second statement per relation.
#![cfg(feature = "sqlx")]

use besu::prelude::*;
use besu::types::Int8;
use besu_sqlite::Sqlx;

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

/// The join table between posts and tags.
///
/// The SQLite schema below deliberately has no primary key on it, so a test can insert the
/// same pair twice and prove the loader folds it into one child.
#[derive(Table, Debug, Clone, PartialEq)]
#[besu(table = "post_tags")]
pub struct PostTag {
    #[besu(primary_key)]
    pub post_id: i64,
    #[besu(primary_key)]
    pub tag_id: i64,
}

/// A relation declared once and reused, which is the point of relation values.
const TAGS: ManyToMany<PostTable, PostTagTable, TagTable, Int8, Int8> =
    many_to_many(posts.id, post_tags.post_id, post_tags.tag_id, tags.id);

async fn setup() -> besu::Database<Sqlx> {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        r#"CREATE TABLE "users" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "email" TEXT NOT NULL UNIQUE,
            "name" TEXT NOT NULL,
            "age" INTEGER NOT NULL,
            "bio" TEXT
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "posts" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "title" TEXT NOT NULL,
            "author_id" INTEGER NOT NULL REFERENCES "users"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "teams" (
            "slug" TEXT PRIMARY KEY,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "members" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "team_slug" TEXT NOT NULL REFERENCES "teams"("slug") ON DELETE CASCADE,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "comments" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "body" TEXT NOT NULL,
            "post_id" INTEGER NOT NULL REFERENCES "posts"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "tags" (
            "id" INTEGER PRIMARY KEY AUTOINCREMENT,
            "name" TEXT NOT NULL
        )"#
    ))
    .await
    .unwrap();
    db.execute(sql!(
        r#"CREATE TABLE "post_tags" (
            "post_id" INTEGER NOT NULL REFERENCES "posts"("id") ON DELETE CASCADE,
            "tag_id" INTEGER NOT NULL REFERENCES "tags"("id") ON DELETE CASCADE
        )"#
    ))
    .await
    .unwrap();
    db
}

/// Three users, posts for the first two, nothing for the third.
async fn seed(db: &besu::Database<Sqlx>) -> Vec<User> {
    let mut parents = Vec::new();
    for (email, name, age) in [
        ("alice@example.com", "Alice", 30),
        ("bob@example.com", "Bob", 20),
        ("carol@example.com", "Carol", 17),
    ] {
        let created = db
            .insert(users)
            .value(users.insert(email, name, age))
            .returning(users)
            .await
            .unwrap();
        parents.extend(created);
    }

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
    let db = setup().await;
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
    let db = besu::dry::<besu_sqlite::SQLite>();

    let statements = db
        .select(users)
        .filter(users.age.gt(18))
        .with(has_many(users.id, posts.author_id))
        .to_sql([3, 5, 3]);

    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[0].sql(),
        r#"SELECT "users"."id", "users"."email", "users"."name", "users"."age", "users"."bio", "users"."id" FROM "users" WHERE ("users"."age" > ?)"#
    );
    assert_eq!(statements[0].args().len(), 1);

    assert_eq!(
        statements[1].sql(),
        r#"SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id" FROM "posts" WHERE ("posts"."author_id" IN (?, ?))"#
    );
    assert_eq!(statements[1].args().len(), 2);
}

#[tokio::test]
async fn no_second_query_when_the_parent_result_is_empty() {
    let db = setup().await;

    // If the relation load sent its child statement, this would fail with "no such table".
    // It must not: an empty parent result means no second round trip at all.
    db.execute(sql!(r#"DROP TABLE "posts""#)).await.unwrap();

    let loaded: Vec<(User, Vec<Post>)> = db
        .select(users)
        .with(has_many(users.id, posts.author_id))
        .await
        .unwrap();

    assert!(loaded.is_empty());
}

#[tokio::test]
async fn with_filter_order_and_pick() {
    let db = setup().await;
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
    let db = setup().await;

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
    let db = setup().await;
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

/// The relation value is declared once at module level and reused; the load is three
/// statements and every parent gets its own children, empty when nothing matched.
#[tokio::test]
async fn many_to_many_loads_children_for_every_parent() {
    let db = setup().await;
    let seeded = seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    assert_eq!(posts_by_id.len(), 3);

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
    // A post with no tags at all, so the empty case is covered in the same result.
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
    // The same tag belongs to two posts, so each owns a copy.
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

/// A duplicated join row is one child, not two: the result is a set of child rows.
#[tokio::test]
async fn many_to_many_folds_duplicate_join_rows() {
    let db = setup().await;
    seed(&db).await;

    let post: Post = db
        .select(posts)
        .order_by(posts.id.asc())
        .first()
        .await
        .unwrap()
        .unwrap();
    let tag = db
        .insert(tags)
        .value(tags.insert("rust"))
        .returning(tags)
        .await
        .unwrap();
    let tag = tag.into_iter().next().unwrap();

    for _ in 0..2 {
        db.insert(post_tags)
            .value(post_tags.insert(post.id, tag.id))
            .await
            .unwrap();
    }

    let loaded: Vec<(Post, Vec<Tag>)> = db
        .select(posts)
        .filter(posts.id.eq(post.id))
        .with(TAGS)
        .await
        .unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, vec![tag]);
}

/// `users -> posts -> comments` is one statement for the parents and one per level, with
/// the shapes nested exactly as the relation values are.
#[tokio::test]
async fn nested_relations_load_one_statement_per_level() {
    let db = setup().await;
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
    assert_eq!(
        loaded[0].1[0]
            .1
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["Alice one first", "Alice one second"]
    );
    assert!(loaded[0].1[1].1.is_empty());
    assert_eq!(loaded[1].1.len(), 1);
    assert_eq!(loaded[1].1[0].1.len(), 1);
    assert!(loaded[2].1.is_empty());
}

/// A level with no rows above it sends no statement at all. Dropping the table proves it:
/// the load cannot have queried it.
#[tokio::test]
async fn a_level_with_nothing_above_it_sends_no_statement() {
    let db = setup().await;
    seed(&db).await;

    db.execute(sql!("DELETE FROM posts")).await.unwrap();
    db.execute(sql!(r#"DROP TABLE "comments""#)).await.unwrap();

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
/// attached, whether the relation is nested or many-to-many.
#[tokio::test]
async fn filters_order_limit_and_pick_still_apply_to_the_parent() {
    let db = setup().await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    let tag = db
        .insert(tags)
        .value(tags.insert("rust"))
        .returning(tags)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    db.insert(post_tags)
        .value(post_tags.insert(posts_by_id[0].id, tag.id))
        .await
        .unwrap();

    // `pick` replaces the row type; the key is appended behind it and stays out of `R`.
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
    assert!(picked[1].1.is_empty());

    // Nested under the same kind of query.
    let nested: Vec<(String, Vec<(Post, Vec<Comment>)>)> = db
        .select(users)
        .filter(users.age.gte(18))
        .order_by(users.name.desc())
        .pick(users.name)
        .with(has_many(users.id, posts.author_id).with(has_many(posts.id, comments.post_id)))
        .await
        .unwrap();
    assert_eq!(
        nested
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Bob", "Alice"]
    );
    assert_eq!(nested[1].1.len(), 2);
    assert_eq!(nested[0].1.len(), 1);
}

/// A many-to-many can hang off a has-many's child table, which is the shape a
/// `users -> posts -> tags` screen needs.
#[tokio::test]
async fn a_many_to_many_can_be_nested_under_a_has_many() {
    let db = setup().await;
    seed(&db).await;

    let posts_by_id: Vec<Post> = db.select(posts).order_by(posts.id.asc()).await.unwrap();
    let tag = db
        .insert(tags)
        .value(tags.insert("rust"))
        .returning(tags)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    for post in &posts_by_id[..2] {
        db.insert(post_tags)
            .value(post_tags.insert(post.id, tag.id))
            .await
            .unwrap();
    }

    let loaded: Vec<(User, Vec<(Post, Vec<Tag>)>)> = db
        .select(users)
        .order_by(users.id.asc())
        .with(has_many(users.id, posts.author_id).with(TAGS))
        .await
        .unwrap();

    assert_eq!(loaded[0].1.len(), 2);
    assert_eq!(loaded[0].1[0].1, vec![tag.clone()]);
    assert_eq!(loaded[0].1[1].1, vec![tag]);
}

/// The limit is per parent, and the order moves within each parent rather than across the
/// whole result: Alice gets her newest post, Bob his only one, Carol still gets an empty
/// `Vec`.
#[tokio::test]
async fn per_parent_limit_and_order_on_has_many() {
    let db = setup().await;
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
    let db = setup().await;
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
    let db = setup().await;
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
    let db = setup().await;
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
    let db = setup().await;
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
    db.insert(posts)
        .value(posts.insert("Bob two", posts_by_id[2].author_id))
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

    // A limit past the child count keeps the whole set, still in the requested order.
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
    let db = setup().await;
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

    // Alice keeps both posts; the newest comment of the first is the only comment kept.
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
    // Bob has one post with one comment; Carol has no posts at all.
    assert_eq!(loaded[1].1.len(), 1);
    assert_eq!(loaded[1].1[0].1[0].body, "Bob one first");
    assert!(loaded[2].1.is_empty());
}

/// The exact SQL a real SQLite load runs: one statement with a windowed derived table.
#[tokio::test]
async fn generated_sql_for_a_per_parent_limit() {
    let db = besu::dry::<besu_sqlite::SQLite>();

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
            r#"SELECT "posts"."id", "posts"."title", "posts"."author_id", "posts"."author_id" "#,
            r#"FROM (SELECT "posts"."id", "posts"."title", "posts"."author_id", "#,
            r#"ROW_NUMBER() OVER (PARTITION BY "posts"."author_id" ORDER BY "posts"."title" DESC) "#,
            r#"AS "besu_row" FROM "posts" WHERE ("posts"."author_id" IN (?, ?))) AS "posts" "#,
            r#"WHERE ("posts"."besu_row" <= ?) ORDER BY "posts"."besu_row" ASC"#
        )
    );
}

/// A limited many-to-many can sit under a limited has-many: each level ranks independently.
#[tokio::test]
async fn a_limited_many_to_many_nests_under_a_limited_has_many() {
    let db = setup().await;
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
    let db = setup().await;
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
