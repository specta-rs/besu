//! Changing a column, which on SQLite means rebuilding the table.
#![cfg(feature = "sqlx")]
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

    let result = db.push_destructive([v2::users.def()]).await.unwrap();

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
            .any(|s| s.starts_with(r#"CREATE TABLE "users__besu_tmp_"#)),
        "{:?}",
        result.statements
    );
    assert!(
        result.statements.iter().any(|s| s
            .starts_with(r#"INSERT INTO "users__besu_tmp_"#)
            && s.ends_with(
                r#"("id", "name", "email", "age", "bio") SELECT "id", "name", "email", "age", "bio" FROM "users""#
            )),
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
            .any(|s| s.starts_with(r#"ALTER TABLE "users__besu_tmp_"#)
                && s.ends_with(r#" RENAME TO "users""#))
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
    let result = db.push_destructive([v2::users.def()]).await.unwrap();
    assert!(
        result
            .statements
            .iter()
            .any(|s| s.starts_with(r#"INSERT INTO "users__besu_tmp_"#)),
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
async fn rebuilding_preserves_autoincrement_after_deleting_the_highest_id() {
    for (before_rebuild, expected) in [
        ("DELETE FROM users WHERE id = 2", 3),
        ("DELETE FROM users", 3),
        // UPDATE can raise a rowid without advancing sqlite_sequence; copying it does.
        ("UPDATE users SET id = 10 WHERE id = 2", 11),
    ] {
        let db = besu::Database::new(Sqlx::memory().await.unwrap());
        db.push([v1::users.def()]).await.unwrap();
        db.execute(sql!(
            "INSERT INTO users (name, email, age, bio) VALUES ('Alice', 'a', 30, ''), ('Bob', 'b', 31, '')"
        ))
        .await
        .unwrap();
        db.execute(besu::Fragment::new(before_rebuild))
            .await
            .unwrap();

        db.push_destructive([v2::users.def()]).await.unwrap();
        let sequence = db
            .execute(sql!("SELECT seq FROM sqlite_sequence WHERE name = 'users'"))
            .first::<i64>()
            .await
            .unwrap();
        assert_eq!(sequence, Some(expected - 1), "{before_rebuild}");

        let id = db
            .execute(sql!(
                "INSERT INTO users (name, email, age) VALUES ('Charlie', 'c', '32') RETURNING id"
            ))
            .first::<i64>()
            .await
            .unwrap();
        assert_eq!(id, Some(expected), "{before_rebuild}");
    }
}

#[tokio::test]
async fn rebuilding_preserves_the_sequence_of_a_qualified_renamed_key() {
    use besu::SchemaDriver;
    use besu::schema::{Ddl, Schema, Table, diff_with};
    use besu_sqlite::SQLiteDdl;

    let driver = Sqlx::memory().await.unwrap();
    let db = besu::Database::new(driver.clone());
    let mut old = Table::from_def(v1::users.def());
    let mut new = Table::from_def(v2::users.def());
    for table in [&mut old, &mut new] {
        table.schema = Some("a\"ux".into());
        table.name = "user's\"rows".into();
        table.indexes.clear();
    }
    let key = new.columns.iter_mut().find(|c| c.primary_key).unwrap();
    key.name = "new_id".into();
    key.renamed_from = Some("id".into());

    driver
        .execute_statements(&[
            r#"ATTACH DATABASE ':memory:' AS "a""ux""#.into(),
            r#"CREATE TABLE "user's""rows" (id INTEGER PRIMARY KEY AUTOINCREMENT)"#.into(),
            r#"INSERT INTO "user's""rows" (id) VALUES (99)"#.into(),
            SQLiteDdl.create_table(&old),
            r#"INSERT INTO "a""ux"."user's""rows" (id, name, email, age, bio) VALUES (4, 'Alice', 'a', 30, '')"#.into(),
            r#"DELETE FROM "a""ux"."user's""rows""#.into(),
        ])
        .await
        .unwrap();

    // SQLite introspection only discovers tables in main, so supply the two schemas to DDL.
    let mut current = Schema::new();
    current.tables.push(old);
    let mut desired = Schema::new();
    desired.tables.push(new);
    let changes = diff_with(&current, &desired, &|ty| SQLiteDdl.normalise_type(ty));
    let statements = SQLiteDdl.apply_with(&changes, &current, &desired);
    driver.execute_statements(&statements).await.unwrap();

    let id = db
        .execute(sql!(
            r#"INSERT INTO "a""ux"."user's""rows" (name, email, age) VALUES ('Bob', 'b', '31') RETURNING new_id"#
        ))
        .first::<i64>()
        .await
        .unwrap();
    assert_eq!(id, Some(5));
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
        .find(|s| s.starts_with(r#"INSERT INTO "users__besu_tmp_"#))
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
        .push_destructive([author_v2::authors.def(), books::books.def()])
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
async fn a_compatible_view_survives_a_rebuild() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_rebuild_parent (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO sqlx_rebuild_parent VALUES (1, 'Ada')"))
        .await
        .unwrap();
    db.execute(sql!(
        "CREATE VIEW sqlx_rebuild_parent_view AS SELECT name FROM sqlx_rebuild_parent"
    ))
    .await
    .unwrap();

    let mut desired = db.introspect().await.unwrap();
    desired.table_mut("sqlx_rebuild_parent").unwrap().columns[1].nullable = true;
    // Relaxing NOT NULL is destructive under the updated safety classifier; opt in so
    // the rebuild can prove the compatible view survives.
    db.push_schema_destructive(&desired).await.unwrap();

    let table_rows: Vec<String> = db
        .execute(sql!("SELECT name FROM sqlx_rebuild_parent"))
        .all()
        .await
        .unwrap();
    assert_eq!(table_rows, ["Ada"]);
    let view_rows: Vec<String> = db
        .execute(sql!("SELECT name FROM sqlx_rebuild_parent_view"))
        .all()
        .await
        .unwrap();
    assert_eq!(table_rows, ["Ada"]);
    assert_eq!(view_rows, ["Ada"]);
    let current = db.introspect().await.unwrap();
    assert!(
        current
            .table("sqlx_rebuild_parent")
            .unwrap()
            .column("name")
            .unwrap()
            .nullable
    );
    assert!(current.view("sqlx_rebuild_parent_view").is_some());
    let legacy: Vec<i64> = db
        .execute(sql!("PRAGMA legacy_alter_table"))
        .all()
        .await
        .unwrap();
    assert_eq!(legacy, [0]);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_incompatible_view_rolls_back_a_rebuild_and_names_the_view() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_incompatible_view_table (id INTEGER PRIMARY KEY, name TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "INSERT INTO sqlx_incompatible_view_table VALUES (1, 'Ada')"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE VIEW sqlx_incompatible_view AS SELECT name FROM sqlx_incompatible_view_table"
    ))
    .await
    .unwrap();
    db.execute(sql!("PRAGMA legacy_alter_table = ON"))
        .await
        .unwrap();

    let mut desired = db.introspect().await.unwrap();
    desired
        .table_mut("sqlx_incompatible_view_table")
        .unwrap()
        .columns
        .remove(1);
    desired
        .table_mut("sqlx_incompatible_view_table")
        .unwrap()
        .checks
        .push(besu::schema::Check {
            name: "sqlx_incompatible_view_table_id_check".into(),
            expression: "id > 0".into(),
        });
    let err = db.push_schema_destructive(&desired).await.unwrap_err();
    assert!(err.to_string().contains("sqlx_incompatible_view"), "{err}");

    let table_rows: Vec<String> = db
        .execute(sql!("SELECT name FROM sqlx_incompatible_view_table"))
        .all()
        .await
        .unwrap();
    let view_rows: Vec<String> = db
        .execute(sql!("SELECT name FROM sqlx_incompatible_view"))
        .all()
        .await
        .unwrap();
    assert_eq!(table_rows, ["Ada"]);
    assert_eq!(view_rows, ["Ada"]);
    let current = db.introspect().await.unwrap();
    assert!(current.table("sqlx_incompatible_view_table").is_some());
    assert!(current.view("sqlx_incompatible_view").is_some());
    let legacy: Vec<i64> = db
        .execute(sql!("PRAGMA legacy_alter_table"))
        .all()
        .await
        .unwrap();
    assert_eq!(legacy, [1]);
    let scratch: Vec<String> = db
        .execute(sql!(
            "SELECT name FROM sqlite_master WHERE name LIKE 'sqlx_incompatible_view_table__besu_tmp_%'"
        ))
        .all()
        .await
        .unwrap();
    assert!(scratch.is_empty(), "{scratch:?}");
}

#[tokio::test]
async fn cancelling_a_rebuild_cannot_return_a_connection_with_foreign_keys_off() {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let dir = tempfile::tempdir().unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(dir.path().join("cancel-rebuild.db"))
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let db = besu::Database::new(Sqlx::from_pool(pool.clone()));
    db.execute(sql!("CREATE TABLE cancel_rebuild (name TEXT NOT NULL)"))
        .await
        .unwrap();
    db.execute(sql!("INSERT INTO cancel_rebuild VALUES ('Ada')"))
        .await
        .unwrap();
    let mut desired = db.introspect().await.unwrap();
    desired.table_mut("cancel_rebuild").unwrap().columns[0].nullable = true;

    let (copying, started) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    {
        let mut copying = Some(copying);
        let mut connection = pool.acquire().await.unwrap();
        connection
            .lock_handle()
            .await
            .unwrap()
            .set_update_hook(move |update| {
                if update.table.starts_with("cancel_rebuild__besu_tmp_")
                    && let Some(copying) = copying.take()
                {
                    let _ = copying.send(());
                    let _ = waiting.recv_timeout(std::time::Duration::from_secs(5));
                }
            });
    }

    let task = tokio::spawn(async move { db.push_schema_destructive(&desired).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();

    let db = besu::Database::new(Sqlx::from_pool(pool));
    let foreign_keys: Vec<i64> = db.execute(sql!("PRAGMA foreign_keys")).all().await.unwrap();
    assert_eq!(foreign_keys, [1]);
    let legacy: Vec<i64> = db
        .execute(sql!("PRAGMA legacy_alter_table"))
        .all()
        .await
        .unwrap();
    assert_eq!(legacy, [0]);
    let rows: Vec<String> = db
        .execute(sql!("SELECT name FROM cancel_rebuild"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, ["Ada"]);
    let current = db.introspect().await.unwrap();
    assert!(
        !current
            .table("cancel_rebuild")
            .unwrap()
            .column("name")
            .unwrap()
            .nullable
    );
    let scratch = db
        .execute(sql!(
            "SELECT name FROM sqlite_master WHERE name LIKE 'cancel_rebuild__besu_tmp_%'"
        ))
        .first::<String>()
        .await
        .unwrap();
    assert_eq!(scratch, None);
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
    db.push_destructive([children_v1::children.def(), parents_v2::parents.def()])
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

#[tokio::test]
async fn adding_a_foreign_key_over_orphan_rows_rolls_back_even_when_enforcement_is_off() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_fk_parents (id INTEGER PRIMARY KEY)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE sqlx_fk_children (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO sqlx_fk_children VALUES (1, 999)"))
        .await
        .unwrap();
    db.execute(sql!("PRAGMA foreign_keys = OFF")).await.unwrap();

    let mut desired = db.introspect().await.unwrap();
    desired
        .table_mut("sqlx_fk_children")
        .unwrap()
        .foreign_keys
        .push(besu::schema::ForeignKey {
            name: "sqlx_fk_children_parent_id_fkey".into(),
            columns: vec!["parent_id".into()],
            references_table: "sqlx_fk_parents".into(),
            references_columns: vec!["id".into()],
            on_delete: besu::schema::ReferenceAction::Default,
            on_update: besu::schema::ReferenceAction::Default,
        });

    let error = db.push_schema(&desired).await.unwrap_err().to_string();
    assert!(error.contains("sqlx_fk_children"), "{error}");
    assert!(error.contains("row 1"), "{error}");
    assert!(error.contains("sqlx_fk_parents"), "{error}");

    let current = db.introspect().await.unwrap();
    assert!(
        current
            .table("sqlx_fk_children")
            .unwrap()
            .foreign_keys
            .is_empty()
    );
    let rows: Vec<(i64, i64)> = db
        .execute(sql!("SELECT id, parent_id FROM sqlx_fk_children"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(1, 999)]);
    let enabled: Vec<i64> = db.execute(sql!("PRAGMA foreign_keys")).all().await.unwrap();
    assert_eq!(enabled, [0]);
    let violations: Vec<(String, i64, String, i64)> = db
        .execute(sql!("PRAGMA foreign_key_check"))
        .all()
        .await
        .unwrap();
    assert!(violations.is_empty());

    db.execute(sql!("UPDATE sqlx_fk_children SET parent_id = 1"))
        .await
        .unwrap();
    db.execute(sql!("INSERT INTO sqlx_fk_parents VALUES (1)"))
        .await
        .unwrap();
    db.push_schema(&desired).await.unwrap();
    let violations: Vec<(String, i64, String, i64)> = db
        .execute(sql!("PRAGMA foreign_key_check"))
        .all()
        .await
        .unwrap();
    assert!(violations.is_empty());
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rebuild_preserves_a_unique_partial_index() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_partial_members (id INTEGER PRIMARY KEY, email TEXT NOT NULL, active INTEGER NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE UNIQUE INDEX sqlx_partial_members_active_email_uq ON sqlx_partial_members (email) WHERE active = 1"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "INSERT INTO sqlx_partial_members VALUES (1, 'Alice@example.com', 1)"
    ))
    .await
    .unwrap();
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_partial_members VALUES (2, 'Alice@example.com', 1)"
        ))
        .await
        .is_err()
    );

    let mut desired = db.introspect().await.unwrap();
    let index = &desired.table("sqlx_partial_members").unwrap().indexes[0];
    assert_eq!(index.name, "sqlx_partial_members_active_email_uq");
    assert!(index.unique);
    assert_eq!(index.predicate.as_deref(), Some("active = 1"));
    desired
        .table_mut("sqlx_partial_members")
        .unwrap()
        .checks
        .push(besu::schema::Check {
            name: "sqlx_partial_members_email_nonempty_check".into(),
            expression: "length(email) > 0".into(),
        });

    let pushed = db.push_schema(&desired).await.unwrap();
    assert!(!pushed.is_destructive());
    let indexes: Vec<(i64, String, i64, String, i64)> = db
        .execute(sql!("PRAGMA index_list('sqlx_partial_members')"))
        .all()
        .await
        .unwrap();
    assert!(indexes.iter().any(|(_, name, unique, _, partial)| {
        name == "sqlx_partial_members_active_email_uq" && *unique == 1 && *partial == 1
    }));
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_partial_members VALUES (2, 'Alice@example.com', 1)"
        ))
        .await
        .is_err()
    );
    let rows: Vec<(i64, String, i64)> = db
        .execute(sql!("SELECT id, email, active FROM sqlx_partial_members"))
        .all()
        .await
        .unwrap();
    assert_eq!(rows, [(1, "Alice@example.com".into(), 1)]);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rebuild_preserves_strict_and_without_rowid() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_strict_items (id INTEGER PRIMARY KEY, qty INTEGER NOT NULL) STRICT"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE sqlx_keyed_items (key TEXT PRIMARY KEY, qty INTEGER NOT NULL) WITHOUT ROWID"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO sqlx_strict_items VALUES (1, 2)"))
        .await
        .unwrap();
    db.execute(sql!("INSERT INTO sqlx_keyed_items VALUES ('one', 2)"))
        .await
        .unwrap();
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_strict_items VALUES (2, 'not an integer')"
        ))
        .await
        .is_err()
    );
    assert!(
        db.execute(sql!("SELECT rowid FROM sqlx_keyed_items"))
            .await
            .is_err()
    );

    let mut desired = db.introspect().await.unwrap();
    assert!(desired.table("sqlx_strict_items").unwrap().strict);
    assert!(desired.table("sqlx_keyed_items").unwrap().without_rowid);
    desired.table_mut("sqlx_strict_items").unwrap().strict = false;
    desired.table_mut("sqlx_keyed_items").unwrap().without_rowid = false;
    desired.table_mut("sqlx_strict_items").unwrap().columns[1].nullable = true;
    desired.table_mut("sqlx_keyed_items").unwrap().columns[1].nullable = true;
    db.push_schema(&desired).await.unwrap();

    let strict_sql: String = db
        .execute(sql!(
            "SELECT sql FROM sqlite_master WHERE name = 'sqlx_strict_items'"
        ))
        .first::<String>()
        .await
        .unwrap()
        .unwrap();
    let keyed_sql: String = db
        .execute(sql!(
            "SELECT sql FROM sqlite_master WHERE name = 'sqlx_keyed_items'"
        ))
        .first::<String>()
        .await
        .unwrap()
        .unwrap();
    assert!(strict_sql.contains("STRICT"), "{strict_sql}");
    assert!(keyed_sql.contains("WITHOUT ROWID"), "{keyed_sql}");
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_strict_items VALUES (2, 'not an integer')"
        ))
        .await
        .is_err()
    );
    assert!(
        db.execute(sql!("SELECT rowid FROM sqlx_keyed_items"))
            .await
            .is_err()
    );
    db.execute(sql!("INSERT INTO sqlx_strict_items VALUES (2, NULL)"))
        .await
        .unwrap();
    db.execute(sql!("INSERT INTO sqlx_keyed_items VALUES ('two', NULL)"))
        .await
        .unwrap();
    let strict_rows: Vec<(i64, Option<i64>)> = db
        .execute(sql!("SELECT id, qty FROM sqlx_strict_items ORDER BY id"))
        .all()
        .await
        .unwrap();
    assert_eq!(strict_rows, [(1, Some(2)), (2, None)]);
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rebuild_preserves_index_collation_and_sort_order() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_accounts (id INTEGER PRIMARY KEY, email TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE UNIQUE INDEX sqlx_accounts_email_nocase_uq ON sqlx_accounts (email COLLATE NOCASE DESC)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE INDEX sqlx_accounts_lower_email_idx ON sqlx_accounts (lower(email) COLLATE NOCASE DESC)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "INSERT INTO sqlx_accounts VALUES (1, 'Alice@example.com')"
    ))
    .await
    .unwrap();
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_accounts VALUES (2, 'alice@example.com')"
        ))
        .await
        .is_err()
    );

    let mut desired = db.introspect().await.unwrap();
    let column = &desired.table("sqlx_accounts").unwrap().indexes[0].columns[0];
    assert_eq!(column.collation.as_deref(), Some("NOCASE"));
    assert!(column.descending);
    let expression = desired
        .table("sqlx_accounts")
        .unwrap()
        .indexes
        .iter()
        .find(|index| index.name == "sqlx_accounts_lower_email_idx")
        .unwrap();
    assert_eq!(expression.columns[0].as_expression(), Some("lower(email)"));
    assert_eq!(expression.columns[0].collation.as_deref(), Some("NOCASE"));
    assert!(expression.columns[0].descending);
    desired.table_mut("sqlx_accounts").unwrap().columns[1].nullable = true;
    let pushed = db.push_schema(&desired).await.unwrap();
    assert!(pushed.statements.iter().any(|statement| {
        statement.contains("sqlx_accounts_email_nocase_uq")
            && statement.contains(r#"COLLATE "NOCASE" DESC"#)
    }));

    let key: Vec<(i64, i64, Option<String>, i64, String, i64)> = db
        .execute(sql!("PRAGMA index_xinfo('sqlx_accounts_email_nocase_uq')"))
        .all()
        .await
        .unwrap();
    assert!(
        key.iter()
            .any(|(_, _, name, descending, collation, is_key)| {
                name.as_deref() == Some("email")
                    && *descending == 1
                    && collation == "NOCASE"
                    && *is_key == 1
            })
    );
    let expression: Vec<(i64, i64, Option<String>, i64, String, i64)> = db
        .execute(sql!("PRAGMA index_xinfo('sqlx_accounts_lower_email_idx')"))
        .all()
        .await
        .unwrap();
    assert!(
        expression
            .iter()
            .any(|(_, _, name, descending, collation, is_key)| {
                name.is_none() && *descending == 1 && collation == "NOCASE" && *is_key == 1
            })
    );
    assert!(
        db.execute(sql!(
            "INSERT INTO sqlx_accounts VALUES (2, 'alice@example.com')"
        ))
        .await
        .is_err()
    );
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rebuild_never_drops_an_unrelated_scratch_suffix_table() {
    let db = besu::Database::new(Sqlx::memory().await.unwrap());
    db.execute(sql!(
        "CREATE TABLE sqlx_scratch_accounts (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!(
        "CREATE TABLE sqlx_scratch_accounts__besu_tmp (id INTEGER PRIMARY KEY, marker TEXT NOT NULL)"
    ))
    .await
    .unwrap();
    db.execute(sql!("INSERT INTO sqlx_scratch_accounts VALUES (1, 10)"))
        .await
        .unwrap();
    db.execute(sql!(
        "INSERT INTO sqlx_scratch_accounts__besu_tmp VALUES (7, 'unrelated application data')"
    ))
    .await
    .unwrap();
    let mut desired = db.introspect().await.unwrap();
    assert!(desired.table("sqlx_scratch_accounts__besu_tmp").is_some());
    desired.table_mut("sqlx_scratch_accounts").unwrap().columns[1].nullable = true;

    let pushed = db.push_schema(&desired).await.unwrap();
    assert!(!pushed.is_destructive());
    let marker: Vec<String> = db
        .execute(sql!("SELECT marker FROM sqlx_scratch_accounts__besu_tmp"))
        .all()
        .await
        .unwrap();
    assert_eq!(marker, ["unrelated application data"]);
    let balance: Vec<Option<i64>> = db
        .execute(sql!("SELECT balance FROM sqlx_scratch_accounts"))
        .all()
        .await
        .unwrap();
    assert_eq!(balance, [Some(10)]);
    let scratch: Vec<String> = db
        .execute(sql!(
            "SELECT name FROM sqlite_master WHERE name LIKE 'sqlx_scratch_accounts__besu_tmp_%'"
        ))
        .all()
        .await
        .unwrap();
    assert!(scratch.is_empty(), "{scratch:?}");
    assert!(db.push_schema(&desired).await.unwrap().is_empty());
}
