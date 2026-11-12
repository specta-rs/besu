//! Deterministic data generation, in the spirit of Drizzle Seed.
//!
//! Seeding a database by hand means writing rows that do not matter to the test: a name here,
//! an email there, a timestamp that only needs to be a timestamp. `seed` fills those in from
//! the table's declaration and inserts the result through the ordinary typed builder, so the
//! rows are checked by the same compiler and the same statement renderer as a hand-written
//! insert.
//!
//! ```ignore
//! let report = seed(&db)
//!     .seed(42)
//!     .table(users, 1_000)
//!     .table(posts, 5_000)
//!     .await?;
//!
//! assert_eq!(report.rows("users"), 1_000);
//! ```
//!
//! # Determinism
//!
//! Every value comes from a SplitMix64 generator seeded by the master seed, the table's name
//! and the row's index. The same seed and the same schema therefore produce the same rows, in
//! the same order, on every run and on every machine — a failing test can be replayed by
//! number rather than by fixture.
//!
//! # Foreign keys
//!
//! Tables are seeded in the order they are listed. Each table records the values it generated
//! for its primary-key and unique columns, and a `references` column picks from the matching
//! pool. Listing a child before its parent is an error, not a silently dangling key:
//! [`SeedError::MissingParent`] names the table that has to be seeded first.
//!
//! A self-referencing table is **not** supported: the key pool for a table is filled as its
//! rows are inserted, and the first row is generated before any key exists. Seed the roots
//! with a plain `insert` first, then call [`seed`] for the rest.
//!
//! # What is not generated
//!
//! Auto-incrementing keys are written explicitly, starting at 1, so a child can refer to
//! them without `RETURNING`. That makes a seed deterministic, but it also means seeding a
//! table that already contains rows will collide with those keys; seed an empty database, or
//! override the key. A column whose type has no generator is left unset and must be
//! overridden with [`TableSeed::set`]; the exact column is listed in
//! [`SeedError::MissingOverride`] rather than left for the database to reject.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use core::{fmt, future::Future, marker::PhantomData, pin::Pin};

use besu_core::{TableDef, TransactionDriver, Value};
use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use uuid::Uuid;

use crate::database::{Database, Transaction};
use crate::expr::IntoSqlValue;
use crate::table::{Insertable, Set, Table};
use crate::types::{
    Array, Bool, Bytes, Char, Column, Date, Decimal, Float4, Float8, I128, Int1, Int2, Int4, Int8,
    Json, Jsonb, Nullability, SqlType, Text, Time, Timestamp, TimestampTz, U128, UInt1, UInt2,
    UInt4, UInt8, Uuid as UuidType, VarChar,
};

/// Start building a deterministic seed.
///
/// The database is borrowed rather than consumed: the whole seed runs in one transaction,
/// which is committed when the returned future resolves and rolled back if any table fails.
/// That matters for foreign keys — a half-seeded parent table is not a state worth leaving
/// behind.
///
/// ```ignore
/// seed(&db).table(users, 1_000).table(posts, 5_000).await?;
/// ```
pub fn seed<Dr: TransactionDriver>(db: &Database<Dr>) -> Seed<'_, Dr> {
    Seed {
        db,
        seed: 0,
        batch_size: 500,
        plan: Vec::new(),
    }
}

/// A seed under construction.
///
/// Add one [`table`](Seed::table) per table, in dependency order, then await the builder.
pub struct Seed<'a, Dr: TransactionDriver> {
    db: &'a Database<Dr>,
    seed: u64,
    batch_size: usize,
    plan: Vec<Box<dyn SeedPlan<Dr>>>,
}

impl<'a, Dr: TransactionDriver> Seed<'a, Dr> {
    /// Set the master seed.
    ///
    /// Zero is the default. The number is mixed with the table name and the row index, so two
    /// tables at the same offset do not receive the same values.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Set the number of rows per `INSERT` statement.
    ///
    /// A batch is one round trip. The default of 500 keeps a wide table under the parameter
    /// limits of every supported database while still amortising the cost of a statement.
    pub fn batch_size(mut self, rows: usize) -> Self {
        self.batch_size = rows.max(1);
        self
    }

    /// Add a table and the number of rows to generate for it.
    ///
    /// Tables are seeded in the order they are added, because a child's foreign keys are
    /// drawn from the keys its parent has already generated. Call
    /// [`TableSeed::set`] on the result to pin a column to a particular value.
    pub fn table<T: Seedable>(self, table: T, rows: usize) -> TableSeed<'a, Dr, T> {
        TableSeed {
            seed: self,
            table,
            rows,
            overrides: Vec::new(),
        }
    }

    /// Run every table that was added.
    pub async fn run(mut self) -> Result<SeedReport, Dr::Error> {
        let db = self.db;
        let mut state = SeedState {
            pools: BTreeMap::new(),
        };
        let plans = &mut self.plan;
        let report = db
            .transaction(async |tx| {
                let mut tables = Vec::with_capacity(plans.len());
                for plan in plans.iter_mut() {
                    let rows = plan.run(tx, &mut state).await?;
                    tables.push(SeedTableReport {
                        table: plan.name(),
                        rows,
                    });
                }
                Ok(SeedReport { tables })
            })
            .await?;
        Ok(report)
    }
}

/// One table in a seed, before it has run.
///
/// Created by [`Seed::table`]. Chain [`TableSeed::table`] to start the next table, or await
/// the builder to run the seed.
pub struct TableSeed<'a, Dr: TransactionDriver, T: Seedable> {
    seed: Seed<'a, Dr>,
    table: T,
    rows: usize,
    overrides: Vec<(&'static str, Value)>,
}

impl<'a, Dr: TransactionDriver, T: Seedable> TableSeed<'a, Dr, T> {
    /// Generate `column` as `value` for every row of this table.
    ///
    /// The value is checked against the column's SQL type, so `set(users.age, "old")` does not
    /// compile and `set(users.bio, None::<String>)` produces an explicit `NULL`. An override
    /// replaces the generated value; it is not appended to it.
    ///
    /// Overriding a unique column to a single value is only useful when the table has one
    /// row: the generator cannot make a fixed value unique.
    pub fn set<S, N>(mut self, column: Column<T, S, N>, value: impl Into<Set<S::Rust>>) -> Self
    where
        S: SqlType,
        N: Nullability,
        S::Rust: IntoSqlValue<S> + Clone,
    {
        if let Some(value) = crate::expr::set_to_value::<S, S::Rust>(&value.into()) {
            let name = column.name();
            self.overrides.retain(|(existing, _)| *existing != name);
            self.overrides.push((name, value));
        }
        self
    }

    /// Set the master seed.
    ///
    /// See [`Seed::seed`]. It applies to the whole seed, not just this table.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed.seed = seed;
        self
    }

    /// Set the number of rows per `INSERT` statement.
    ///
    /// See [`Seed::batch_size`]. It applies to the whole seed, not just this table.
    pub fn batch_size(mut self, rows: usize) -> Self {
        self.seed.batch_size = rows.max(1);
        self
    }

    /// Add the next table.
    ///
    /// The current table is fixed when the next one is added, so the chain runs in the order
    /// it is written.
    pub fn table<U: Seedable>(self, table: U, rows: usize) -> TableSeed<'a, Dr, U> {
        self.finish().table(table, rows)
    }

    /// Run the seed.
    pub async fn run(self) -> Result<SeedReport, Dr::Error> {
        self.finish().run().await
    }

    /// Close this table and hand the accumulated plan back.
    fn finish(mut self) -> Seed<'a, Dr> {
        let plan = TablePlan {
            table: self.table,
            rows: self.rows,
            overrides: self.overrides,
            batch_size: self.seed.batch_size,
            master_seed: self.seed.seed,
        };
        self.seed.plan.push(Box::new(plan));
        self.seed
    }
}

impl<'a, Dr: TransactionDriver, T: Seedable> IntoFuture for TableSeed<'a, Dr, T> {
    type Output = Result<SeedReport, Dr::Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// What a completed seed did.
///
/// One entry per table, in the order the tables were listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedReport {
    tables: Vec<SeedTableReport>,
}

impl SeedReport {
    /// The number of rows generated for `table`, or zero if it was not part of the seed.
    pub fn rows(&self, table: &str) -> usize {
        self.tables
            .iter()
            .find(|entry| entry.table == table)
            .map_or(0, |entry| entry.rows)
    }

    /// Every table's contribution, in the order it was seeded.
    pub fn tables(&self) -> impl Iterator<Item = &SeedTableReport> {
        self.tables.iter()
    }

    /// The total number of rows generated across every table.
    pub fn total(&self) -> usize {
        self.tables.iter().map(|entry| entry.rows).sum()
    }
}

/// One table's contribution to a [`SeedReport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedTableReport {
    table: &'static str,
    rows: usize,
}

impl SeedTableReport {
    /// The table's SQL name.
    pub fn table(&self) -> &'static str {
        self.table
    }

    /// The number of rows generated for it.
    pub fn rows(&self) -> usize {
        self.rows
    }
}

/// Why a seed could not generate its rows.
///
/// These are converted into the driver's error type when the seed runs, so a caller sees one
/// error type as with any other statement.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeedError {
    /// A foreign key references a table that has not been seeded yet.
    ///
    /// List the parent before the child: the child's keys are drawn from the parent's
    /// generated rows, and there is no pool until the parent has run.
    MissingParent {
        /// The referenced table's name.
        table: &'static str,
        /// The referenced column's name.
        column: &'static str,
    },
    /// A parent table was seeded, but with no rows to draw a key from.
    EmptyParent {
        /// The referenced table's name.
        table: &'static str,
        /// The referenced column's name.
        column: &'static str,
    },
    /// The parent's key is a different SQL type from the child's column.
    ParentTypeMismatch {
        /// The referenced table's name.
        table: &'static str,
        /// The referenced column's name.
        column: &'static str,
        /// The child column's name.
        child: &'static str,
    },
    /// An override names a column that is not part of the table's insert.
    UnknownColumn {
        /// The table being seeded.
        table: &'static str,
        /// The column name that was given.
        column: &'static str,
    },
    /// A column has no generator and no override was provided.
    MissingOverride {
        /// The table being seeded.
        table: &'static str,
        /// The column that needs a value.
        column: &'static str,
    },
}

impl fmt::Display for SeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParent { table, column } => write!(
                f,
                "cannot seed a foreign key referencing `{table}.{column}`: seed `{table}` first"
            ),
            Self::EmptyParent { table, column } => write!(
                f,
                "cannot seed a foreign key referencing `{table}.{column}`: `{table}` was seeded \
                 with zero rows"
            ),
            Self::ParentTypeMismatch {
                table,
                column,
                child,
            } => write!(
                f,
                "cannot use `{table}.{column}` as the source of `{child}`: the key's SQL type \
                 does not match the column's"
            ),
            Self::UnknownColumn { table, column } => {
                write!(f, "`{table}` has no insertable column named `{column}`")
            }
            Self::MissingOverride { table, column } => write!(
                f,
                "`{table}.{column}` has no generator; give it a value with `.set(..)`"
            ),
        }
    }
}

impl std::error::Error for SeedError {}

impl From<SeedError> for besu_core::Error {
    fn from(err: SeedError) -> Self {
        Self::invalid_query(err.to_string())
    }
}

/// A table whose rows the derive can generate.
///
/// Implemented by `#[derive(Table)]` for every table. The generated method is called once per
/// row, with a context that owns that row's generator and a read-only view of the keys
/// already produced by earlier tables.
#[doc(hidden)]
pub trait Seedable: Table {
    /// Columns with no generator that the caller must supply.
    ///
    /// A custom `sql_type` has no sane default, so its column is left unset. This list names
    /// the ones that are also required by SQL — not nullable, no default and not
    /// auto-incrementing — so the seed can fail with a useful message instead of a
    /// `NOT NULL` violation.
    const REQUIRED: &'static [&'static str] = &[];

    /// Generate one row.
    fn seed_row(ctx: &mut SeedContext<'_>) -> Result<Self::Insert, SeedError>;
}

/// The per-row state the generated `Seedable` implementation reads.
#[doc(hidden)]
pub struct SeedContext<'a> {
    rng: &'a mut Rng,
    row: usize,
    pools: &'a BTreeMap<(String, String), Vec<Value>>,
}

impl SeedContext<'_> {
    /// The zero-based index of the row being generated.
    pub fn row(&self) -> usize {
        self.row
    }

    /// Generate an ordinary value for `name`.
    pub fn value<S: SeedValue>(&mut self, name: &'static str) -> S::Rust {
        S::generate(self.rng, &SeedColumn::new(name, self.row, false))
    }

    /// Generate a value that is unique across the table for `name`.
    pub fn unique<S: SeedValue>(&mut self, name: &'static str) -> S::Rust {
        S::generate_unique(self.rng, &SeedColumn::new(name, self.row, true))
    }

    /// Generate an auto-incrementing or primary-key value for `name`.
    pub fn primary<S: SeedValue>(&mut self, name: &'static str) -> S::Rust {
        S::generate_primary(self.rng, &SeedColumn::new(name, self.row, true))
    }

    /// Generate a nullable value, which is `NULL` roughly a quarter of the time.
    pub fn optional<S: SeedValue>(&mut self, name: &'static str) -> Set<S::Rust> {
        if self.rng.chance(1, 4) {
            Set::Null
        } else {
            Set::Value(self.value::<S>(name))
        }
    }

    /// Pick a key generated for `table.column`, for the child column `child`.
    ///
    /// The pool is the set of primary-key and unique values the parent table actually wrote,
    /// so every returned key satisfies the foreign key. An absent pool is an error rather
    /// than a made-up key.
    pub fn foreign_key<S: SeedValue>(
        &mut self,
        child: &'static str,
        table: &'static str,
        column: &'static str,
    ) -> Result<S::Rust, SeedError> {
        let pool = lookup(self.pools, table, column);
        let pool = match pool {
            Some(pool) if !pool.is_empty() => pool,
            Some(_) => {
                return Err(SeedError::EmptyParent { table, column });
            }
            None => {
                return Err(SeedError::MissingParent { table, column });
            }
        };
        let index = self.rng.below(pool.len());
        S::from_value(&pool[index]).ok_or(SeedError::ParentTypeMismatch {
            table,
            column,
            child,
        })
    }
}

/// Find a parent's key pool, tolerating a schema-qualified reference.
fn lookup<'a>(
    pools: &'a BTreeMap<(String, String), Vec<Value>>,
    table: &str,
    column: &str,
) -> Option<&'a Vec<Value>> {
    if let Some(pool) = pools.get(&(table.to_string(), column.to_string())) {
        return Some(pool);
    }
    // `references = "audit.users.id"` names a table in another schema; the pool is keyed by
    // the table's declared name, which is the last segment.
    if let Some(short) = table.rsplit('.').next() {
        return pools.get(&(short.to_string(), column.to_string()));
    }
    None
}

/// The description of a column a generator is filling.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct SeedColumn {
    /// The column's SQL name. Generators use it as a hint: an `email` column gets an address
    /// and a `name` column gets a name.
    pub name: &'static str,
    /// The zero-based row index, used both as the unique discriminator and by
    /// auto-incrementing keys.
    pub row: usize,
    /// Whether the value must not repeat within the table.
    pub unique: bool,
}

impl SeedColumn {
    /// Describe `name` for the row at `row`.
    pub fn new(name: &'static str, row: usize, unique: bool) -> Self {
        Self { name, row, unique }
    }
}

/// A generator for one [`SqlType`].
///
/// Implemented for every built-in type in this module. The derive names an implementation
/// for each column, so a custom SQL type simply has no `SeedValue` and is left for the caller
/// to override.
#[doc(hidden)]
pub trait SeedValue: SqlType {
    /// Generate an ordinary value.
    fn generate(rng: &mut Rng, column: &SeedColumn) -> Self::Rust;

    /// Generate a value that does not repeat within the table.
    ///
    /// The default is good enough for types that are unique by construction, such as
    /// [`Uuid`], and is overridden by the integer and text generators.
    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
        Self::generate(rng, column)
    }

    /// Generate a primary-key value.
    ///
    /// Integers start at 1 so a parent's keys are small and predictable; every other type
    /// falls back to [`SeedValue::generate_unique`].
    fn generate_primary(rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
        Self::generate_unique(rng, column)
    }

    /// Recover a generated value from a parent's key pool.
    ///
    /// `None` means the parent's key and this column's type are not the same kind of value,
    /// which is reported rather than coerced.
    fn from_value(value: &Value) -> Option<Self::Rust>;
}

/// SplitMix64.
///
/// A seed needs a generator that is small, fast and identical everywhere; this is the
/// reference SplitMix64 mixing function, which has no state beyond a `u64` and needs no
/// dependency.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Start a generator from `seed`.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.state)
    }

    /// A number below `bound`, or zero when `bound` is zero.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    /// True with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        denominator != 0 && self.next_u64() % denominator < numerator
    }

    /// One random byte.
    pub fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xff) as u8
    }
}

/// The SplitMix64 finaliser, also used to mix the seed with a table and row.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Mix the master seed, the table name and the row into one generator seed.
///
/// Each row gets its own stream, so its values do not depend on how many rows preceded it or
/// on the order the tables were listed — only on the seed and the row's identity.
fn row_seed(master: u64, table: &str, row: usize) -> u64 {
    // FNV-1a over the table name. `DefaultHasher` is not promised to be stable between
    // compiler versions, and a seed that changes with a toolchain upgrade is not a seed.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in table.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    mix(hash ^ master ^ mix(row as u64))
}

/// The state shared by every table in one seed.
struct SeedState {
    pools: BTreeMap<(String, String), Vec<Value>>,
}

/// A table's contribution, erased over the table's type.
trait SeedPlan<Dr: TransactionDriver> {
    /// The table's SQL name.
    fn name(&self) -> &'static str;

    /// Generate and insert this table's rows.
    fn run<'a, 'tx: 'a>(
        &'a mut self,
        tx: &'a mut Transaction<'tx, Dr>,
        state: &'a mut SeedState,
    ) -> Pin<Box<dyn Future<Output = Result<usize, Dr::Error>> + 'a>>;
}

/// A table waiting to run.
struct TablePlan<T: Seedable> {
    table: T,
    rows: usize,
    overrides: Vec<(&'static str, Value)>,
    batch_size: usize,
    master_seed: u64,
}

impl<Dr: TransactionDriver, T: Seedable> SeedPlan<Dr> for TablePlan<T> {
    fn name(&self) -> &'static str {
        self.table.def().name
    }

    fn run<'a, 'tx: 'a>(
        &'a mut self,
        tx: &'a mut Transaction<'tx, Dr>,
        state: &'a mut SeedState,
    ) -> Pin<Box<dyn Future<Output = Result<usize, Dr::Error>> + 'a>> {
        let rows = self.rows;
        let overrides = &self.overrides;
        let batch_size = self.batch_size;
        let master_seed = self.master_seed;
        Box::pin(async move {
            // Fail before generating anything when a required column has neither a
            // generator nor an override, so the message names the column rather than
            // surfacing as a constraint violation from the database.
            for required in T::REQUIRED {
                if !overrides.iter().any(|(name, _)| name == required) {
                    return Err(Dr::error(
                        SeedError::MissingOverride {
                            table: T::DEF.name,
                            column: required,
                        }
                        .into(),
                    ));
                }
            }

            let mut generated = Vec::with_capacity(rows);
            for row in 0..rows {
                let mut rng = Rng::new(row_seed(master_seed, T::DEF.name, row));
                let values = {
                    let mut ctx = SeedContext {
                        rng: &mut rng,
                        row,
                        pools: &state.pools,
                    };
                    let mut values = T::seed_row(&mut ctx)
                        .map_err(|err| Dr::error(err.into()))?
                        .into_values();
                    apply_overrides(
                        &mut values,
                        T::DEF.name,
                        <T::Insert as Insertable>::COLUMNS,
                        overrides,
                    )
                    .map_err(|err| Dr::error(err.into()))?;
                    values
                };
                record_keys(
                    &mut state.pools,
                    T::DEF,
                    <T::Insert as Insertable>::COLUMNS,
                    &values,
                );
                generated.push(SeedRow::<T::Insert> {
                    values: pad_to_row(T::DEF, <T::Insert as Insertable>::COLUMNS, values),
                    phantom: PhantomData,
                });
            }

            let count = generated.len();
            let insert: crate::query::Insert<
                '_,
                &mut Transaction<'tx, Dr>,
                (),
                SeedRow<T::Insert>,
            > = crate::query::Insert::new(tx, T::DEF);
            insert.values_chunked(generated, batch_size).await?;
            Ok(count)
        })
    }
}

/// Expand an insert struct's values to one entry per table column.
///
/// The insert struct omits columns the database computes, while the statement's value buffer
/// is indexed by [`TableDef::columns`]. Padding the computed positions with `None` keeps the
/// two aligned; the insert builder leaves an all-`None` column out of the statement, which is
/// exactly the behaviour a generated column needs.
fn pad_to_row(
    def: &'static TableDef,
    columns: &'static [&'static str],
    values: Vec<Option<Value>>,
) -> Vec<Option<Value>> {
    if def.columns.len() == columns.len() {
        return values;
    }

    let mut values = values.into_iter();
    let mut out = Vec::with_capacity(def.columns.len());
    for column in def.columns {
        if columns.contains(&column.name) {
            out.push(values.next().unwrap_or(None));
        } else {
            out.push(None);
        }
    }
    out
}

/// A row already converted to bound values, so an override can be applied by name.
struct SeedRow<I: Insertable> {
    values: Vec<Option<Value>>,
    phantom: PhantomData<fn() -> I>,
}

impl<I: Insertable> Insertable for SeedRow<I> {
    const COLUMNS: &'static [&'static str] = I::COLUMNS;

    fn into_values(self) -> Vec<Option<Value>> {
        self.values
    }

    fn write_values(self, out: &mut Vec<Option<Value>>) {
        out.extend(self.values);
    }
}

/// Replace generated values with the caller's overrides.
fn apply_overrides(
    values: &mut [Option<Value>],
    table: &'static str,
    columns: &'static [&'static str],
    overrides: &[(&'static str, Value)],
) -> Result<(), SeedError> {
    for (name, value) in overrides {
        match columns.iter().position(|column| column == name) {
            Some(index) => values[index] = Some(value.clone()),
            None => {
                return Err(SeedError::UnknownColumn {
                    table,
                    column: name,
                });
            }
        }
    }
    Ok(())
}

/// Remember the keys a table just wrote, so its children can refer to them.
fn record_keys(
    pools: &mut BTreeMap<(String, String), Vec<Value>>,
    def: &'static TableDef,
    columns: &'static [&'static str],
    values: &[Option<Value>],
) {
    for (name, value) in columns.iter().zip(values) {
        let Some(value) = value else { continue };
        if value.is_null() {
            continue;
        }
        let Some(column) = def.columns.iter().find(|column| column.name == *name) else {
            continue;
        };
        if column.primary_key || column.unique {
            pools
                .entry((def.name.to_string(), (*name).to_string()))
                .or_default()
                .push(value.clone());
        }
    }
}

// ---------------------------------------------------------------------------------------
// Built-in generators
// ---------------------------------------------------------------------------------------

macro_rules! integer_seed {
    ($name:ident, $rust:ty, $base:expr, $span:expr) => {
        impl SeedValue for $name {
            fn generate(rng: &mut Rng, _column: &SeedColumn) -> Self::Rust {
                $base + ((rng.next_u64() % $span) as $rust)
            }

            fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
                // The row number itself, not a random value plus the row number: adding two
                // rings together can collide (1 + 10 equals 10 + 1), and a unique integer
                // that collides on the second row is not unique.
                (column.row as $rust).wrapping_add(1)
            }

            fn generate_primary(_rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
                (column.row as $rust).wrapping_add(1)
            }

            fn from_value(value: &Value) -> Option<Self::Rust> {
                match value {
                    Value::Int(value) => <$rust>::try_from(*value).ok(),
                    Value::UInt(value) => <$rust>::try_from(*value).ok(),
                    Value::UInt128(value) => <$rust>::try_from(*value).ok(),
                    Value::Int128(value) => <$rust>::try_from(*value).ok(),
                    _ => None,
                }
            }
        }
    };
}

integer_seed!(Int1, i8, 1, 100);
integer_seed!(Int2, i16, 1, 10_000);
integer_seed!(Int4, i32, 1, 1_000_000);
integer_seed!(Int8, i64, 1, 1_000_000);
integer_seed!(UInt1, u8, 1, 200);
integer_seed!(UInt2, u16, 1, 50_000);
integer_seed!(UInt4, u32, 1, 1_000_000);
integer_seed!(UInt8, u64, 1, 1_000_000);
integer_seed!(U128, u128, 1, 1_000_000);
integer_seed!(I128, i128, 1, 1_000_000);

impl SeedValue for Bool {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> bool {
        rng.chance(1, 2)
    }

    fn from_value(value: &Value) -> Option<bool> {
        match value {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

macro_rules! float_seed {
    ($name:ident, $rust:ty) => {
        impl SeedValue for $name {
            fn generate(rng: &mut Rng, _column: &SeedColumn) -> Self::Rust {
                (rng.next_u64() % 1_000_000) as $rust / 100.0
            }

            fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
                column.row as $rust + 1.0
            }

            fn generate_primary(rng: &mut Rng, column: &SeedColumn) -> Self::Rust {
                <Self as SeedValue>::generate_unique(rng, column)
            }

            fn from_value(value: &Value) -> Option<Self::Rust> {
                match value {
                    Value::Float(value) => Some(*value as $rust),
                    Value::Int(value) => Some(*value as $rust),
                    Value::UInt(value) => Some(*value as $rust),
                    _ => None,
                }
            }
        }
    };
}

float_seed!(Float4, f32);
float_seed!(Float8, f64);

impl SeedValue for Text {
    fn generate(rng: &mut Rng, column: &SeedColumn) -> String {
        text_value(rng, column, None)
    }

    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> String {
        text_value(rng, column, None)
    }

    fn generate_primary(rng: &mut Rng, column: &SeedColumn) -> String {
        text_value(rng, column, None)
    }

    fn from_value(value: &Value) -> Option<String> {
        match value {
            Value::Text(value) | Value::Decimal(value) => Some(value.clone()),
            _ => None,
        }
    }
}

macro_rules! string_length_seed {
    ($name:ident, $len:ident) => {
        impl<const $len: u32> SeedValue for $name<$len> {
            fn generate(rng: &mut Rng, column: &SeedColumn) -> String {
                text_value(rng, column, Some($len as usize))
            }

            fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> String {
                text_value(rng, column, Some($len as usize))
            }

            fn generate_primary(rng: &mut Rng, column: &SeedColumn) -> String {
                text_value(rng, column, Some($len as usize))
            }

            fn from_value(value: &Value) -> Option<String> {
                match value {
                    Value::Text(value) | Value::Decimal(value) => Some(value.clone()),
                    _ => None,
                }
            }
        }
    };
}

string_length_seed!(Char, N);
string_length_seed!(VarChar, N);

impl<const PRECISION: u8, const SCALE: u8> SeedValue for Decimal<PRECISION, SCALE> {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> String {
        decimal_text(rng, SCALE, None)
    }

    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> String {
        // A unique decimal is told apart by its integer part, which is the row number. The
        // fractional digits stay random so the value still looks like a measurement.
        decimal_text(rng, SCALE, Some(column.row))
    }

    fn generate_primary(rng: &mut Rng, column: &SeedColumn) -> String {
        <Self as SeedValue>::generate_unique(rng, column)
    }

    fn from_value(value: &Value) -> Option<String> {
        match value {
            Value::Decimal(value) | Value::Text(value) => Some(value.clone()),
            _ => None,
        }
    }
}

impl SeedValue for Bytes {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> Vec<u8> {
        (0..8).map(|_| rng.next_u8()).collect()
    }

    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> Vec<u8> {
        // The row number in the first eight bytes is what makes the value unique; the rest
        // keeps it looking like data rather than a counter.
        let mut bytes = column.row.to_le_bytes().to_vec();
        bytes.extend((0..8).map(|_| rng.next_u8()));
        bytes
    }

    fn from_value(value: &Value) -> Option<Vec<u8>> {
        match value {
            Value::Bytes(value) => Some(value.clone()),
            _ => None,
        }
    }
}

/// The start of the date window, picked so a seeded date never moves with the clock.
fn date_from_days(days: u64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2015, 1, 1)
        .and_then(|date| date.checked_add_days(Days::new(days)))
        .unwrap_or(NaiveDate::MIN)
}

impl SeedValue for Date {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> NaiveDate {
        date_from_days(rng.next_u64() % 3_650)
    }

    fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> NaiveDate {
        // A ten-year window has 3,650 distinct days; a table larger than that cannot have a
        // unique date, and the insert reports the collision.
        date_from_days(column.row as u64 % 3_650)
    }

    fn from_value(value: &Value) -> Option<NaiveDate> {
        match value {
            Value::Date(value) => Some(*value),
            _ => None,
        }
    }
}

impl SeedValue for Time {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> NaiveTime {
        let seconds = (rng.next_u64() % 86_400) as u32;
        NaiveTime::from_num_seconds_from_midnight_opt(seconds, 0).unwrap_or(NaiveTime::MIN)
    }

    fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> NaiveTime {
        let seconds = (column.row as u64 % 86_400) as u32;
        NaiveTime::from_num_seconds_from_midnight_opt(seconds, 0).unwrap_or(NaiveTime::MIN)
    }

    fn from_value(value: &Value) -> Option<NaiveTime> {
        match value {
            Value::Time(value) => Some(*value),
            _ => None,
        }
    }
}

/// 2020-01-01T00:00:00Z, the start of the timestamp window.
const TIMESTAMP_EPOCH: i64 = 1_577_836_800;

/// Ten years in seconds.
const TIMESTAMP_SPAN: u64 = 315_360_000;

impl SeedValue for Timestamp {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> NaiveDateTime {
        let seconds = rng.next_u64() % TIMESTAMP_SPAN;
        DateTime::from_timestamp(TIMESTAMP_EPOCH + seconds as i64, 0)
            .map(|value| value.naive_utc())
            .unwrap_or(NaiveDateTime::MIN)
    }

    fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> NaiveDateTime {
        let seconds = column.row as u64 % TIMESTAMP_SPAN;
        DateTime::from_timestamp(TIMESTAMP_EPOCH + seconds as i64, 0)
            .map(|value| value.naive_utc())
            .unwrap_or(NaiveDateTime::MIN)
    }

    fn from_value(value: &Value) -> Option<NaiveDateTime> {
        match value {
            Value::Timestamp(value) => Some(*value),
            _ => None,
        }
    }
}

impl SeedValue for TimestampTz {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> DateTime<Utc> {
        let seconds = rng.next_u64() % TIMESTAMP_SPAN;
        DateTime::from_timestamp(TIMESTAMP_EPOCH + seconds as i64, 0)
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    fn generate_unique(_rng: &mut Rng, column: &SeedColumn) -> DateTime<Utc> {
        let seconds = column.row as u64 % TIMESTAMP_SPAN;
        DateTime::from_timestamp(TIMESTAMP_EPOCH + seconds as i64, 0)
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    fn from_value(value: &Value) -> Option<DateTime<Utc>> {
        match value {
            Value::TimestampTz(value) => Some(*value),
            _ => None,
        }
    }
}

impl SeedValue for UuidType {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> Uuid {
        let mut bytes = [0_u8; 16];
        for byte in &mut bytes {
            *byte = rng.next_u8();
        }
        // A version-4 shape, so the value survives any check that expects a real UUID rather
        // than sixteen arbitrary bytes.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid::from_bytes(bytes)
    }

    fn from_value(value: &Value) -> Option<Uuid> {
        match value {
            Value::Uuid(value) => Some(*value),
            _ => None,
        }
    }
}

impl SeedValue for Json {
    fn generate(rng: &mut Rng, _column: &SeedColumn) -> serde_json::Value {
        serde_json::json!({
            "id": rng.next_u64() % 1_000_000,
            "active": rng.chance(1, 2),
            "label": text_value(rng, &SeedColumn::new("label", 0, false), None),
        })
    }

    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> serde_json::Value {
        // The id is the discriminator. Two rows cannot produce the same JSON text when the
        // ids differ, which keeps a unique JSON column honest without a canonical encoder.
        let mut value = <Self as SeedValue>::generate(rng, column);
        if let serde_json::Value::Object(object) = &mut value {
            object.insert("id".into(), serde_json::Value::from(column.row as u64 + 1));
        }
        value
    }

    fn from_value(value: &Value) -> Option<serde_json::Value> {
        match value {
            Value::Json(value) => serde_json::from_slice(value).ok(),
            _ => None,
        }
    }
}

impl SeedValue for Jsonb {
    fn generate(rng: &mut Rng, column: &SeedColumn) -> serde_json::Value {
        <Json as SeedValue>::generate(rng, column)
    }

    fn generate_unique(rng: &mut Rng, column: &SeedColumn) -> serde_json::Value {
        <Json as SeedValue>::generate_unique(rng, column)
    }

    fn from_value(value: &Value) -> Option<serde_json::Value> {
        <Json as SeedValue>::from_value(value)
    }
}

impl<S: SeedValue> SeedValue for Array<S>
where
    Array<S>: SqlType<Rust = Vec<S::Rust>>,
{
    fn generate(_rng: &mut Rng, _column: &SeedColumn) -> Self::Rust {
        // An empty array is the one value every array column can hold and every reader can
        // interpret. A non-empty one needs a generator for the element type and a sense of
        // how many elements the caller wants, which a column declaration does not carry.
        Vec::new()
    }

    fn from_value(value: &Value) -> Option<Self::Rust> {
        match value {
            Value::Array { values, .. } => values.iter().map(S::from_value).collect(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Text generation
// ---------------------------------------------------------------------------------------

const FIRST_NAMES: &[&str] = &[
    "Ada", "Alan", "Alice", "Barbara", "Carol", "Donald", "Edsger", "Frances", "Grace", "Hedy",
    "Ivan", "Jean", "Ken", "Linus", "Margaret", "Niklaus", "Oscar", "Radia", "Tim", "Yukihiro",
];

const LAST_NAMES: &[&str] = &[
    "Allen",
    "Berners",
    "Church",
    "Dijkstra",
    "Floyd",
    "Hopper",
    "Johnson",
    "Knuth",
    "Lamport",
    "Liskov",
    "Lovelace",
    "McCarthy",
    "Milner",
    "Perlman",
    "Ritchie",
    "Shannon",
    "Stroustrup",
    "Thompson",
    "Torvalds",
    "Wirth",
];

const DOMAINS: &[&str] = &["example.com", "example.org", "example.net", "test.invalid"];

const WORDS: &[&str] = &[
    "amber", "bridge", "cinder", "dawn", "ember", "forest", "glimmer", "harbour", "island",
    "jasper", "kernel", "lantern", "meadow", "north", "ocean", "pebble", "quiet", "river",
    "signal", "timber", "umber", "valley", "willow", "xenon", "yonder", "zephyr",
];

/// Pick a word from a list.
fn pick(rng: &mut Rng, list: &'static [&'static str]) -> &'static str {
    list[rng.below(list.len())]
}

/// Generate text, using the column's name as a hint and honouring a length limit.
fn text_value(rng: &mut Rng, column: &SeedColumn, max: Option<usize>) -> String {
    let first = pick(rng, FIRST_NAMES);
    let last = pick(rng, LAST_NAMES);
    let row = column.row + 1;
    let name = column.name;

    if name.contains("email") || name.contains("mail") {
        if column.unique {
            let domain = pick(rng, DOMAINS);
            let address = alloc::format!(
                "{}.{}{}@{}",
                first.to_lowercase(),
                last.to_lowercase(),
                row,
                domain
            );
            // A short `VARCHAR` must not truncate away the counter that makes the value
            // unique, so drop the decorative parts before the number.
            let address = match max {
                Some(max) if max < address.chars().count() => alloc::format!("u{row}@{domain}"),
                _ => address,
            };
            let address = match max {
                Some(max) if max < address.chars().count() => alloc::format!("u{row}"),
                _ => address,
            };
            return clamp(address, max);
        }
        let local = alloc::format!("{}.{}", first.to_lowercase(), last.to_lowercase());
        return clamp(alloc::format!("{}@{}", local, pick(rng, DOMAINS)), max);
    }

    let base = if name.contains("name") {
        alloc::format!("{first} {last}")
    } else if name.contains("slug") || name.contains("code") {
        alloc::format!("{}-{}", first.to_lowercase(), last.to_lowercase())
    } else {
        sentence(rng)
    };

    if !column.unique {
        return clamp(base, max);
    }

    let suffix = if base.ends_with('.') {
        alloc::format!(" ({row})")
    } else {
        alloc::format!("-{row}")
    };

    match max {
        Some(max) if max <= suffix.chars().count() => clamp(row.to_string(), Some(max)),
        Some(max) => {
            let room = max - suffix.chars().count();
            alloc::format!("{}{}", clamp(base, Some(room)), suffix)
        }
        None => alloc::format!("{base}{suffix}"),
    }
}

/// A short sentence built from the word list.
fn sentence(rng: &mut Rng) -> String {
    let count = 4 + rng.below(5);
    let mut out = String::new();
    for index in 0..count {
        if index != 0 {
            out.push(' ');
        }
        out.push_str(pick(rng, WORDS));
    }
    let mut chars = out.chars();
    if let Some(first) = chars.next() {
        out = first.to_uppercase().collect::<String>() + chars.as_str();
    }
    out.push('.');
    out
}

/// Truncate to `max` characters, counting characters rather than bytes.
fn clamp(text: String, max: Option<usize>) -> String {
    let Some(max) = max else {
        return text;
    };
    if text.chars().count() <= max {
        return text;
    }
    text.chars().take(max).collect()
}

/// Render an exact decimal with `scale` digits after the point.
///
/// `unique_row` makes the integer part the row number, which is what keeps a unique decimal
/// column distinct without abandoning the requested scale.
fn decimal_text(rng: &mut Rng, scale: u8, unique_row: Option<usize>) -> String {
    let whole = match unique_row {
        Some(row) => (row + 1) as u64,
        None => 1 + rng.next_u64() % 100_000,
    };
    if scale == 0 {
        return whole.to_string();
    }

    let digits = usize::from(scale.min(18));
    let fraction = rng.next_u64() % 10_u64.pow(digits as u32);
    let mut fraction = alloc::format!("{fraction:0width$}", width = digits);
    while fraction.len() < usize::from(scale) {
        fraction.push('0');
    }
    alloc::format!("{whole}.{fraction}")
}
