use proc_macro2::{Ident, Span, TokenStream};
use quote::ToTokens;

pub struct ConstIdent(&'static str);

impl ConstIdent {
    pub const fn new(s: &'static str) -> Self {
        Self(s)
    }
}

impl ToTokens for ConstIdent {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        Ident::new(self.0, Span::call_site()).to_tokens(tokens)
    }
}

// // `Iterator::unzip` but allowing for more than 2 outputs
// pub fn unzip<T: Clone + 'static, const N: usize>(
//     iter: impl Iterator<Item = [T; N]>,
// ) -> [impl Iterator<Item = T>; N] {
//     let result = iter.collect::<Vec<_>>();

//     let mut i = 0;
//     // let result = &result;
//     [(); N].map(move |_| {
//         let iter = result.iter();
//         let index = i;
//         i += 1;
//         iter.map(move |arr| arr[index].clone())
//     })
// }
