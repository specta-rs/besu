//! Compile-time checks on which tables a query may refer to.
//!
//! A query's "row set" — the type `M` that follows every builder around — describes exactly
//! which tables are in scope. It is a tuple, built up as joins are added:
//!
//! ```text
//! db.select(user)                          // M = (User,)
//!    .left_join(post, ...)                 // M = ((User,), Option<Post>)
//!    .inner_join(comment, ...)             // M = (((User,), Option<Post>), Comment)
//! ```
//!
//! [`Contains`] proves that a table appears somewhere in that tuple, including behind an
//! [`Option`] introduced by an outer join. Nesting is unbounded, so any number of joins
//! works, and referring to a table that was never joined simply does not compile.
//!
//! The `Path` parameter exists only so the compiler can search a tuple: without it the
//! blanket implementations for the head and the tail of a tuple would overlap. Every
//! consumer makes `Path` an inferred generic, so users never spell it out.

use core::marker::PhantomData;

/// The first element of a tuple.
pub enum Zero {}

/// The second element of a tuple.
pub enum One {}

/// A path that descends into the first element of a tuple.
pub struct First<P>(PhantomData<P>);

/// A path that descends into the second element of a tuple.
pub struct Second<P>(PhantomData<P>);

/// The path used by expressions and literals, which reference no table of their own.
pub enum LiteralPath {}

/// A marker used in place of a table by expressions and literal values.
///
/// Nothing ever inserts a `Literal` into a row set; it only exists so that the containment
/// check for a literal is trivially true.
pub enum Literal {}

/// `Container` contains `X` somewhere, reachable by `Path`.
pub trait Contains<Path, X> {}

impl<A> Contains<Zero, A> for (A,) {}
impl<A, B> Contains<Zero, A> for (A, B) {}
impl<A, B> Contains<One, B> for (A, B) {}
impl<P, I, A: Contains<P, I>, B> Contains<First<P>, I> for (A, B) {}
impl<P, I, A, B: Contains<P, I>> Contains<Second<P>, I> for (A, B) {}

impl<A> Contains<(), A> for Option<A> {}

// Literals are accepted by every row set. The path is a distinct type from `Zero`, `One`,
// `First<_>` and `Second<_>` so this never overlaps with the structural implementations
// above.
impl<M> Contains<LiteralPath, Literal> for M {}

/// Two column types must line up for a comparison.
///
/// `(A, B): SameType` is only implemented when the two are equal, which lets the compiler
/// *infer* the right-hand type from the left-hand one: `eq(users.id, 5)` unifies the
/// literal's type with the column's, so `5` is checked as `i64` rather than silently
/// widened or coerced.
pub trait SameType {}

impl<T> SameType for (T, T) {}
