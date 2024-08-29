//! Besu Macros - The macros used by the Besu library.
#![cfg_attr(docsrs, feature(doc_cfg))]

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput};
use util::ConstIdent;

mod table;
mod util;

pub(crate) static CRATE_NAME: ConstIdent = ConstIdent::new("besu");

/// TODO
#[proc_macro_derive(Table)]
pub fn table(input: TokenStream) -> TokenStream {
    match table::table(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error().into(),
    }
}
