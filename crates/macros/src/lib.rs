use proc_macro::TokenStream;
use util::ConstIdent;

mod table;
mod util;

static CRATE_NAME: ConstIdent = ConstIdent::new("besu");

#[proc_macro]
pub fn table(tokens: TokenStream) -> TokenStream {
    table::table(tokens)
}
