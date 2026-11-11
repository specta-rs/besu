//! Projections.
//!
//! A [`Selection`] describes what a `SELECT` returns. The tuple you select *is* the shape of
//! the result, so `pick((users.id, users.name))` yields `Vec<(i64, String)>` with no
//! hand-written row mapping anywhere.

use alloc::vec::Vec;

use crate::types::{Column, Nullability, SqlType};

use crate::condition::{Combine, Contains, LiteralPath, PathNullability};
use crate::expr::{Expr, ExprKind};
use crate::table::Table;

/// The columns a `SELECT` should return, and how to decode them.
///
/// `M` is the query's row set and `P` is a search path for the containment check; both are
/// inferred, so you never write them.
pub trait Selection<M, P = LiteralPath> {
    /// The Rust type produced for one row.
    type Out;

    /// Render the projection, returning the expressions and the number of columns they
    /// produce.
    fn into_projection(self) -> (Vec<ExprKind>, usize);
}

impl<M, S: SqlType> Selection<M, LiteralPath> for Expr<M, S> {
    type Out = S::Rust;

    fn into_projection(self) -> (Vec<ExprKind>, usize) {
        (alloc::vec![self.into_kind()], 1)
    }
}

impl<M, T, Path> Selection<M, Path> for T
where
    M: Contains<Path, T>,
    T: Table,
    Path: PathNullability,
{
    // The outer side of a join yields `None` for the whole row, so selecting the table there
    // has to widen to `Option<T::Model>` just like a single column does.
    type Out = <Path::Nullability as Nullability>::Wrap<T::Model>;

    fn into_projection(self) -> (Vec<ExprKind>, usize) {
        let mut exprs = Vec::with_capacity(T::DEF.columns.len());
        for column in T::DEF.columns {
            exprs.push(ExprKind::Column {
                table: T::DEF.name,
                column: column.name,
            });
        }
        (exprs, T::DEF.columns.len())
    }
}

impl<M, T, S, N, P> Selection<M, P> for Column<T, S, N>
where
    M: Contains<P, T>,
    T: Table,
    S: SqlType,
    N: Nullability,
    P: PathNullability,
    N: Combine<<P as PathNullability>::Nullability>,
{
    // A column from the outer side of a `LEFT JOIN` may be `NULL` even when the column is
    // `NOT NULL`, so the projection widens to `Option<_>` on its own.
    type Out =
        <<N as Combine<<P as PathNullability>::Nullability>>::Out as Nullability>::Wrap<S::Rust>;

    fn into_projection(self) -> (Vec<ExprKind>, usize) {
        (
            alloc::vec![ExprKind::Column {
                table: T::DEF.name,
                column: self.name(),
            }],
            1,
        )
    }
}

macro_rules! impl_tuple_selection {
    ($($name:ident : $path:ident),+) => {
        impl<M, $($name, $path,)+> Selection<M, ($($path,)+)> for ($($name,)+)
        where
            $($name: Selection<M, $path>,)+
        {
            type Out = ($($name::Out,)+);

            #[allow(non_snake_case)]
            fn into_projection(self) -> (Vec<ExprKind>, usize) {
                let ($($name,)+) = self;
                let mut exprs = Vec::new();
                let mut columns = 0;
                $(
                    let (mut inner, count) = $name.into_projection();
                    exprs.append(&mut inner);
                    columns += count;
                )+
                (exprs, columns)
            }
        }
    };
}

impl_tuple_selection!(A: PA);
impl_tuple_selection!(A: PA, B: PB);
impl_tuple_selection!(A: PA, B: PB, C: PC);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG, H: PH);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG, H: PH, I: PI);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG, H: PH, I: PI, J: PJ);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG, H: PH, I: PI, J: PJ, K: PK);
impl_tuple_selection!(A: PA, B: PB, C: PC, D: PD, E: PE, F: PF, G: PG, H: PH, I: PI, J: PJ, K: PK, L: PL);
