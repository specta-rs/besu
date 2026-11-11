//! Projections.
//!
//! A [`Selection`] describes what a `SELECT` returns. The tuple you select *is* the shape of
//! the result, so `pick((users.id, users.name))` yields `Vec<(i64, String)>` with no
//! hand-written row mapping anywhere.

use alloc::vec::Vec;

use crate::types::{Column, Nullability, SqlType};

use crate::condition::{Contains, LiteralPath};
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

impl<M, T, S, N, P> Selection<M, P> for Column<T, S, N>
where
    M: Contains<P, T>,
    T: Table,
    S: SqlType,
    N: Nullability,
{
    type Out = N::Wrap<S::Rust>;

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
