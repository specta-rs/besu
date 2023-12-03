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
