use proc_macro::TokenStream;
use quote::ToTokens;

mod field;
mod field_schema;
mod table;

pub fn table(tokens: TokenStream) -> TokenStream {
    syn::parse_macro_input!(tokens as table::Table)
        .into_token_stream()
        .into()
}
