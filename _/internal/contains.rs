use std::{convert::Infallible, marker::PhantomData};

pub enum Zero {}
pub enum One {}

pub struct First<P>(PhantomData<P>, Infallible);
pub struct Second<P>(PhantomData<P>, Infallible);

// TODO: Explain this trait and the fact that it doesn't support `T` but can be worked around by using `(T, ())` cause infinite recursion is supported.
pub trait Contains<P, T> {}

impl<A> Contains<Zero, A> for (A,) {}

impl<A> Contains<(), A> for Option<A> {}

impl<P, I, A: Contains<P, I>> Contains<First<P>, I> for (A,) {}

impl<A, B> Contains<Zero, A> for (A, B) {}
impl<A, B> Contains<One, B> for (A, B) {}

impl<P, I, A: Contains<P, I>, B> Contains<First<P>, I> for (A, B) {}
impl<P, I, A, B: Contains<P, I>> Contains<Second<P>, I> for (A, B) {}

#[cfg(test)]
#[allow(unused)]
mod tests {
    use super::*;

    pub enum A {}

    const fn contains_a<P, T: Contains<P, A>>() {}

    // Will only compile if type is a tuple containing A
    const _: () = {
        // contains_a::<_, ()>(); // won't compile
        contains_a::<_, (A,)>();
        contains_a::<_, (A, ())>();
        contains_a::<_, ((), A)>();
        contains_a::<_, ((), (A, ()))>();
        contains_a::<_, ((), ((), A))>();
        contains_a::<_, ((), ((A, ()), ()))>();
        contains_a::<_, ((), (((), A), ()))>();
        contains_a::<_, ((), ((), ((), A)))>();
        contains_a::<_, ((), ((), (A, ())))>();
        // contains_a::<_, ((), ((), (u32, ())))>(); // won't compile

        // `Option` are a special case and passthough to the inner type.
        contains_a::<_, (Option<A>,)>();
        contains_a::<_, ((), ((), (Option<A>, ())))>();
    };
}
