//! Benchmarks for the PostgreSQL driver's encoding and decoding paths.
//!
//! The `encode` group needs no server: it exercises the code that turns a finished
//! [`besu_core::Sql`] into `sqlx` arguments. The `query` group runs against a real server and
//! is registered only when `BESU_POSTGRES_BENCH_URL` is set, so a checkout without a database
//! can still run `cargo bench -p besu-postgres`.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use std::hint::black_box;

use besu::Executor;
use besu::prelude::*;
use besu_core::{Decode, Sql, Value};
use besu_postgres::{Postgres, Sqlx, encode_arguments};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

/// A row shape wide enough that a decoded row is a mix of fixed-width, variable-width and
/// nullable columns.
#[derive(Table, Debug, Clone)]
#[besu(table = "besu_driver_bench")]
pub struct BenchRow {
    #[besu(primary_key, auto_increment)]
    id: i64,
    name: String,
    score: i64,
    flag: bool,
    note: Option<String>,
}

/// A row whose values all arrive as `NUMERIC`, so the benchmark isolates the hand-written
/// binary decoder rather than the network.
#[derive(Table, Debug, Clone)]
#[besu(table = "besu_driver_bench_numeric")]
pub struct NumericRow {
    #[besu(primary_key)]
    id: i64,
    big: u64,
    wider: u128,
    signed: i128,
}

/// A statement whose parameters cover every wire shape the driver encodes differently.
fn representative_statement() -> Sql<Postgres> {
    let mut sql = Sql::<Postgres>::raw(
        "INSERT INTO bench (a, b, c, d, e, f, g, h, i, j, k, l) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    );
    sql.bind(7i32);
    sql.bind(i64::MIN + 1);
    sql.bind(18_446_744_073_709_551_615u64);
    sql.bind(340_282_366_920_938_463_463_374_607_431_768_211_455u128);
    sql.bind("a moderately sized piece of text, long enough to be interesting");
    sql.bind(vec![0u8; 64]);
    sql.bind(uuid::Uuid::from_u128(
        0x0123_4567_89ab_cdef_0123_4567_89ab_cdef,
    ));
    sql.bind(1.5f64);
    sql.bind(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap());
    sql.bind_value(Value::Json(
        br#"{"name":"bench","tags":["a","b","c"],"nested":{"n":1,"ok":true}}"#.to_vec(),
    ));
    sql.bind_value(Value::Null);
    sql.bind(true);
    sql.seal()
}

/// Encode a JSON document several times larger than a scalar, so the difference between
/// parsing it into a tree and copying the bytes shows up clearly.
fn json_statement() -> Sql<Postgres> {
    let mut sql = Sql::<Postgres>::raw("INSERT INTO bench (document) VALUES ($1)");
    sql.bind_value(Value::Json(
        br#"{"users":[{"id":1,"name":"Oscar","email":"oscar@example.com","roles":["admin","dev"],"active":true},{"id":2,"name":"Ada","email":"ada@example.com","roles":["dev"],"active":false},{"id":3,"name":"Grace","email":"grace@example.com","roles":["ops"],"active":true}],"meta":{"page":1,"per_page":3,"total":3}}"#.to_vec(),
    ));
    sql.seal()
}

/// A statement whose parameters are the three integers PostgreSQL spells `NUMERIC`.
///
/// `u64` above `i64::MAX`, `u128` and `i128` all have to reach the server as `NUMERIC`, so
/// this isolates that encode path from the rest of the mix.
fn numeric_statement() -> Sql<Postgres> {
    let mut sql = Sql::<Postgres>::raw("INSERT INTO bench (a, b, c) VALUES ($1, $2, $3)");
    sql.bind(18_446_744_073_709_551_615u64);
    sql.bind(340_282_366_920_938_463_463_374_607_431_768_211_455u128);
    sql.bind(i128::MIN);
    sql.seal()
}

fn encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode");

    let statement = representative_statement();
    group.bench_function("mixed", |b| {
        b.iter(|| black_box(encode_arguments(black_box(&statement)).unwrap()));
    });

    let numeric = numeric_statement();
    group.bench_function("numeric", |b| {
        b.iter(|| black_box(encode_arguments(black_box(&numeric)).unwrap()));
    });

    let json = json_statement();
    group.bench_function("json", |b| {
        b.iter(|| black_box(encode_arguments(black_box(&json)).unwrap()));
    });

    group.finish();
}

fn query(c: &mut Criterion) {
    let Ok(url) = std::env::var("BESU_POSTGRES_BENCH_URL") else {
        eprintln!(
            "BESU_POSTGRES_BENCH_URL is not set; skipping the `query` benchmarks \
             (for example BESU_POSTGRES_BENCH_URL=postgres://besu:besu@localhost:55432/besu)"
        );
        return;
    };

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let db = runtime.block_on(async {
        let db = besu::Database::new(Sqlx::new(&url).await.unwrap());
        // Create the table directly rather than through `push`: the server is shared with the
        // integration tests, and a schema push would try to reconcile every table they left
        // behind. `select` only needs the table definition, never the migrated schema.
        db.execute(sql!("DROP TABLE IF EXISTS besu_driver_bench"))
            .await
            .unwrap();
        db.execute(sql!(
            "CREATE TABLE besu_driver_bench (\
             id BIGSERIAL PRIMARY KEY, \
             name TEXT NOT NULL, \
             score BIGINT NOT NULL, \
             flag BOOLEAN NOT NULL, \
             note TEXT)"
        ))
        .await
        .unwrap();
        db
    });

    // A thousand rows is large enough that per-row costs dominate the round trip, which is
    // the number this benchmark is meant to track.
    runtime.block_on(async {
        db.insert(besu_driver_bench)
            .values((0..1_000).map(|index| {
                besu_driver_bench.insert(format!("row {index}"), i64::from(index), index % 2 == 0)
            }))
            .await
            .unwrap();
    });

    let mut group = c.benchmark_group("query");

    group.bench_function("select_1000_rows", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let rows: Vec<BenchRow> = db.select(besu_driver_bench).limit(1_000).await.unwrap();
                black_box(rows)
            })
        });
    });

    // Fetch the rows once so this benchmark measures only the driver's decode path. The
    // round trip is dominated by the server and by whatever else the machine is doing, which
    // hides changes of a few nanoseconds per column.
    let rows = runtime
        .block_on(async {
            let mut exec = &db;
            exec.query(&Sql::<Postgres>::raw(
                "SELECT id, name, score, flag, note FROM besu_driver_bench LIMIT 1000",
            ))
            .await
        })
        .unwrap();
    group.bench_function("decode_1000_rows", |b| {
        b.iter(|| {
            let mut decoded = Vec::with_capacity(rows.len());
            for row in &rows {
                let mut offset = 0;
                decoded.push(<BenchRow as Decode<Sqlx>>::decode(row, &mut offset).unwrap());
            }
            black_box(decoded)
        });
    });

    // The same isolation for the hand-written `NUMERIC` reader, which every `u64`, `u128`
    // and `i128` column goes through.
    let numeric_rows = runtime
        .block_on(async {
            db.execute(sql!("DROP TABLE IF EXISTS besu_driver_bench_numeric"))
                .await
                .unwrap();
            db.execute(sql!(
                "CREATE TABLE besu_driver_bench_numeric (id BIGINT PRIMARY KEY, \
                 big NUMERIC(20,0) NOT NULL, wider NUMERIC(39,0) NOT NULL, \
                 signed NUMERIC(39,0) NOT NULL)"
            ))
            .await
            .unwrap();
            db.insert(besu_driver_bench_numeric)
                .values((0..1_000i64).map(|index| {
                    besu_driver_bench_numeric.insert(
                        index,
                        u64::MAX - index as u64,
                        u128::MAX - index as u128,
                        i128::MIN + index as i128,
                    )
                }))
                .await
                .unwrap();
            let mut exec = &db;
            exec.query(&Sql::<Postgres>::raw(
                "SELECT id, big, wider, signed FROM besu_driver_bench_numeric ORDER BY id",
            ))
            .await
        })
        .unwrap();
    group.bench_function("decode_numeric_1000_rows", |b| {
        b.iter(|| {
            let mut decoded = Vec::with_capacity(numeric_rows.len());
            for row in &numeric_rows {
                let mut offset = 0;
                decoded.push(<NumericRow as Decode<Sqlx>>::decode(row, &mut offset).unwrap());
            }
            black_box(decoded)
        });
    });

    let mut next = 0i64;
    group.bench_function("insert_one_row", |b| {
        b.iter_batched(
            || {
                next += 1;
                db.insert(besu_driver_bench)
                    .value(besu_driver_bench.insert("inserted", next, true))
            },
            |insert| {
                runtime.block_on(async {
                    black_box(insert.await.unwrap());
                })
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    runtime.block_on(async {
        db.execute(sql!("DROP TABLE IF EXISTS besu_driver_bench"))
            .await
            .unwrap();
        db.execute(sql!("DROP TABLE IF EXISTS besu_driver_bench_numeric"))
            .await
            .unwrap();
    });
}

criterion_group!(driver, encode, query);
criterion_main!(driver);
