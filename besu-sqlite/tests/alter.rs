//! Changing a column, which on SQLite means rebuilding the table.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]
// The derive emits a module with `use super::*;`, and when its model lives in another
// module the compiler reports that glob as an unused import of the model.
#![allow(unused_imports)]

use besu::prelude::*;
use besu_sqlite::Sqlx;

mod parents_v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "parents")]
    pub struct Parent {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
        pub note: i32,
    }
}

mod parents_v2 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "parents")]
    pub struct Parent {
        #[besu(primary_key)]
        pub id: i64,
        pub name: String,
        /// `INTEGER` before, `TEXT` now: this is what forces the rebuild.
        pub note: String,
    }
}

mod children_v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone)]
    #[besu(table = "children")]
    pub struct Child {
        #[besu(primary_key)]
        pub id: i64,
        #[besu(references = "parents.id", on_delete = "cascade")]
        pub parent_id: i64,
    }
}

mod v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub name: String,
        #[besu(index)]
        pub email: String,
        pub age: i32,
        pub bio: String,
    }
}

mod v2 {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub name: String,
        #[besu(index)]
        pub email: String,
        /// `INTEGER NOT NULL` before; `TEXT` now.
        pub age: String,
        /// `NOT NULL` before.
        pub bio: Option<String>,
    }
}

mod v3 {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "users")]
    pub struct User {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        /// `NOT NULL` before.
        pub name: Option<String>,
        #[besu(index)]
        pub email: String,
        pub age: String,
        /// Added by the same push as the rebuild.
        #[besu(default = "anon")]
        pub nickname: String,
        // `bio` is gone by the time this schema is pushed.
    }
}

#[tokio::test]
async fn altering_a_column_rebuilds_the_table_without_losing_rows() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([v1::users.def()]).await.unwrap();
    db.execute(sql!(
        "INSERT INTO users (id, name, email, age, bio) VALUES (?, ?, ?, ?, ?)",
        1,
        "Alice",
        "alice@example.com",
        30,
        "Rust"
    ))
    .await
    .unwrap();

    let result = db.push([v2::users.def()]).await.unwrap();

    // A type change and a nullability change are two `AlterColumn`s, but one rebuild.
    assert_eq!(result.changes.len(), 2);
    assert_eq!(result.statements[0], "PRAGMA foreign_keys = OFF");
    assert_eq!(
        result.statements.last().unwrap(),
        "PRAGMA foreign_keys = ON",
        "foreign keys must be enforceable again: {:?}",
        result.statements
    );
    assert!(
        result
            .statements
            .iter()
            .any(|s| s.starts_with(r#"CREATE TABLE "users__besu_tmp""#)),
        "{:?}",
        result.statements
    );
    assert!(
        result.statements.iter().any(|s| s
            == r#"INSERT INTO "users__besu_tmp" ("id", "name", "email", "age", "bio") SELECT "id", "name", "email", "age", "bio" FROM "users""#),
        "every shared column must be copied by name: {:?}",
        result.statements
    );
    assert!(
        result
            .statements
            .iter()
            .any(|s| s == r#"DROP TABLE "users""#)
    );
    assert!(
        result
            .statements
            .iter()
            .any(|s| s == r#"ALTER TABLE "users__besu_tmp" RENAME TO "users""#)
    );

    // The row survived, and SQLite converted the integer to the column's new TEXT affinity.
    let rows = db
        .execute(sql!("SELECT id, name, email, age, bio FROM users"))
        .all::<(i64, String, String, String, Option<String>)>()
        .await
        .unwrap();
    assert_eq!(
        rows,
        [(
            1,
            "Alice".to_string(),
            "alice@example.com".to_string(),
            "30".to_string(),
            Some("Rust".to_string()),
        )]
    );

    // The schema matches what was asked for, and the index came back with it.
    let schema = db.introspect().await.unwrap();
    let users = schema.table("users").unwrap();
    assert_eq!(users.column("age").unwrap().ty, besu::TypeDef::Text);
    assert!(users.column("bio").unwrap().nullable);
    assert!(!users.column("name").unwrap().nullable);
    assert!(
        users.indexes.iter().any(|i| i.name == "users_email_idx"),
        "the declared index must be recreated: {:?}",
        users.indexes
    );

    let again = db.push([v2::users.def()]).await.unwrap();
    assert!(
        again.is_empty(),
        "the rebuilt schema must converge, got {:?}",
        again.changes
    );
}

#[tokio::test]
async fn rebuilding_an_empty_table_copies_nothing() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([v1::users.def()]).await.unwrap();

    // The copy is still emitted, but selecting from an empty table is not an error.
    let result = db.push([v2::users.def()]).await.unwrap();
    assert!(
        result
            .statements
            .iter()
            .any(|s| s.starts_with(r#"INSERT INTO "users__besu_tmp""#)),
        "{:?}",
        result.statements
    );

    let rows = db
        .execute(sql!("SELECT COUNT(*) FROM users"))
        .all::<(i64,)>()
        .await
        .unwrap();
    assert_eq!(rows, [(0,)]);

    let again = db.push([v2::users.def()]).await.unwrap();
    assert!(again.is_empty(), "got {:?}", again.changes);
}

#[tokio::test]
async fn a_rebuild_absorbs_the_other_changes_for_the_same_table() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([v1::users.def()]).await.unwrap();
    db.execute(sql!(
        "INSERT INTO users (id, name, email, age, bio) VALUES (?, ?, ?, ?, ?)",
        1,
        "Alice",
        "alice@example.com",
        30,
        "Rust"
    ))
    .await
    .unwrap();

    // This push adds `nickname`, drops `bio` and alters `name`, all at once. The rebuild
    // already produces the desired table, so the add and the drop must not run separately:
    // the added column must be omitted from the copy (and take its default), and the
    // dropped one must not be selected.
    let desired = besu::Database::<Sqlx>::schema([v3::users.def()]);
    let result = db.push_schema_destructive(&desired).await.unwrap();

    let insert = result
        .statements
        .iter()
        .find(|s| s.starts_with(r#"INSERT INTO "users__besu_tmp""#))
        .unwrap_or_else(|| panic!("no copy statement in {:?}", result.statements));
    assert!(
        insert.contains(r#""age""#) && insert.contains(r#""name""#),
        "the altered and untouched columns must be copied: {insert}"
    );
    assert!(
        !insert.contains(r#""nickname""#),
        "an added column must not be selected: {insert}"
    );
    assert!(
        !insert.contains(r#""bio""#),
        "a dropped column must not be selected: {insert}"
    );
    assert!(
        !result
            .statements
            .iter()
            .any(|s| s.contains("ADD COLUMN") || s.contains("DROP COLUMN")),
        "those changes are part of the rebuild: {:?}",
        result.statements
    );

    let rows = db
        .execute(sql!("SELECT name, nickname FROM users"))
        .all::<(String, String)>()
        .await
        .unwrap();
    assert_eq!(rows, [("Alice".to_string(), "anon".to_string())]);

    let schema = db.introspect().await.unwrap();
    let users = schema.table("users").unwrap();
    assert!(users.column("name").unwrap().nullable);
    assert!(users.column("bio").is_none());
    assert!(users.column("nickname").is_some());
    assert!(
        users.indexes.iter().any(|i| i.name == "users_email_idx"),
        "the declared index must survive a drop-and-add push: {:?}",
        users.indexes
    );

    let again = db.push_schema(&desired).await.unwrap();
    assert!(
        again.is_empty(),
        "the rebuilt schema must converge, got {:?}",
        again.changes
    );
}

mod author_v1 {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "authors")]
    pub struct Author {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub name: String,
    }
}

mod author_v2 {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "authors")]
    pub struct Author {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        /// `NOT NULL` before, which forces a rebuild.
        pub name: Option<String>,
    }
}

mod books {
    use besu::Table;

    #[derive(Table, Debug, Clone, PartialEq)]
    #[besu(table = "books")]
    pub struct Book {
        #[besu(primary_key, auto_increment)]
        pub id: i64,
        pub title: String,
        #[besu(references = "authors.id", on_delete = "cascade")]
        pub author_id: i64,
    }
}

#[tokio::test]
async fn rebuilding_a_parent_table_does_not_disturb_its_children() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.push([author_v1::authors.def(), books::books.def()])
        .await
        .unwrap();
    db.execute(sql!(
        "INSERT INTO authors (id, name) VALUES (?, ?)",
        1,
        "Alice"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "INSERT INTO books (title, author_id) VALUES (?, ?)",
        "Hello",
        1
    ))
    .await
    .unwrap();

    let result = db
        .push([author_v2::authors.def(), books::books.def()])
        .await
        .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert!(result.statements[0].starts_with("PRAGMA foreign_keys = OFF"));

    // With foreign keys left on, dropping the parent would have cascaded into the child.
    let authors = db
        .execute(sql!("SELECT COUNT(*) FROM authors"))
        .all::<(i64,)>()
        .await
        .unwrap();
    let books = db
        .execute(sql!("SELECT COUNT(*) FROM books"))
        .all::<(i64,)>()
        .await
        .unwrap();
    assert_eq!(authors, [(1,)]);
    assert_eq!(books, [(1,)]);

    // And the constraint is still enforced against the rebuilt table.
    let error = db
        .execute(sql!(
            "INSERT INTO books (title, author_id) VALUES (?, ?)",
            "Orphan",
            999
        ))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("foreign key"),
        "expected a foreign key violation, got: {error}"
    );

    let again = db
        .push([author_v2::authors.def(), books::books.def()])
        .await
        .unwrap();
    assert!(
        again.is_empty(),
        "the rebuilt schema must converge, got {:?}",
        again.changes
    );
}

#[tokio::test]
async fn a_rebuild_keeps_its_foreign_key_toggle_on_one_connection() {
    // A file database pools several connections, which is how the foreign-key pragma could
    // end up applied to a different connection than the `DROP TABLE` it is meant to protect.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    let db = besu::Database::new(Sqlx::new(&path).await.unwrap());

    db.push([children_v1::children.def()]).await.unwrap();
    db.push([children_v1::children.def(), parents_v1::parents.def()])
        .await
        .unwrap();
    db.execute(sql!(
        "INSERT INTO parents (id, name, note) VALUES (1, 'parent', 7)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO children (id, parent_id) VALUES (1, 1)"))
        .await
        .unwrap();

    // Change a column on the parent, which forces a rebuild of the table the child points at.
    db.push([children_v1::children.def(), parents_v2::parents.def()])
        .await
        .unwrap();

    let children: Vec<(i64,)> = db
        .execute(sql!("SELECT id FROM children"))
        .all()
        .await
        .unwrap();
    assert_eq!(
        children,
        [(1,)],
        "a cascading delete would have removed the child row"
    );

    // And the constraint is enforced again afterwards.
    assert!(
        db.execute(sql!("INSERT INTO children (id, parent_id) VALUES (2, 999)"))
            .await
            .is_err()
    );
}
