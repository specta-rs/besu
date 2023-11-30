use proc_macro::TokenStream;
use quote::quote;

#[proc_macro]
pub fn table_v2(_tokens: TokenStream) -> TokenStream {
    quote! {
        todo!();
    }
    .into()
}
