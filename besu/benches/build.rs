//! How long it takes to turn a typed query into SQL, and how often that allocates.
//!
//! Run with `cargo bench -p besu`. The numbers are for building only — no database is
//! involved, which is the point: this is the cost Besu adds on top of the driver, and it is
//! the cost a `to_sql` caller pays for a dry run or a log line.
//!
//! `to_sql` takes `&self` so that rendering a query leaves the builder usable, which means it
//! clones the query first. Every execution path moves instead, so a real query pays slightly
//! less than these numbers.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::borrow::Cow;

use besu::prelude::*;
use besu_core::{Dialect, Placeholder, TypeDef, UpsertStyle, Value};
use criterion::{Criterion, criterion_group, criterion_main};

/// A minimal dialect, because a driver crate depends on `besu` and cannot be a dev-dependency
/// of it. It renders the way SQLite does; the exact spelling does not affect the timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bench;

impl Dialect for Bench {
    type Value = Value;

    const NAME: &'static str = "bench";
    const PLACEHOLDER: Placeholder = Placeholder::Question;
    const SUPPORTS_RETURNING: bool = true;
    const SUPPORTS_ILIKE: bool = false;
    const SUPPORTS_FULL_JOIN: bool = true;
    const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

    fn bind(value: Value) -> Result<Self::Value, besu::Error> {
        Ok(value)
    }

    fn write_placeholder(out: &mut String, index: usize) {
        let _ = index;
        out.push('?');
    }

    fn type_name(ty: TypeDef) -> Cow<'static, str> {
        Cow::Borrowed(match ty {
            TypeDef::Bool => "BOOLEAN",
            TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 => "INTEGER",
            TypeDef::Text => "TEXT",
            _ => "BLOB",
        })
    }
}

#[derive(Table, Debug, Clone)]
#[besu(table = "users")]
pub struct User {
    #[besu(primary_key, auto_increment)]
    pub id: i64,
    pub email: String,
    pub name: String,
    pub age: i32,
    pub bio: Option<String>,
}

fn build(c: &mut Criterion) {
    let db = besu::dry::<Bench>();
    let mut group = c.benchmark_group("build");

    group.bench_function("select 1 filter", |b| {
        b.iter(|| db.select(users).filter(users.age.gt(18)).to_sql().unwrap());
    });

    group.bench_function("select 3 filters + order + limit", |b| {
        b.iter(|| {
            db.select(users)
                .filter(users.age.gt(18))
                .filter(users.name.like("A%"))
                .filter(users.bio.is_null())
                .order_by(users.name.asc())
                .limit(50)
                .to_sql()
                .unwrap()
        });
    });

    group.bench_function("select projection + count", |b| {
        b.iter(|| {
            db.select(users)
                .pick((users.id, count_all()))
                .to_sql()
                .unwrap()
        });
    });

    group.bench_function("insert single", |b| {
        b.iter(|| {
            db.insert(users)
                .value(users.insert("a@example.com", "Alice", 30))
                .to_sql()
                .unwrap()
        });
    });

    group.bench_function("insert 100 rows", |b| {
        let rows: Vec<_> = (0..100)
            .map(|i| users.insert(format!("a{i}@example.com"), "Alice", 30))
            .collect();
        b.iter(|| {
            db.insert(users)
                .values(rows.iter().cloned())
                .to_sql()
                .unwrap()
        });
    });

    group.bench_function("update set + filter", |b| {
        b.iter(|| {
            db.update(users)
                .set(users.name, "Bob")
                .filter(users.id.eq(1))
                .to_sql()
                .unwrap()
        });
    });

    group.finish();
}

criterion_group!(benches, build);
criterion_main!(benches);
