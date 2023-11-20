pub(crate) enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L: Clone, R: Clone> Clone for Either<L, R> {
    fn clone(&self) -> Self {
        match self {
            Either::Left(l) => Either::Left(l.clone()),
            Either::Right(r) => Either::Right(r.clone()),
        }
    }
}
