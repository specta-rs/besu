use syn::{
    parse::{Parse, ParseStream},
    Ident,
};

use super::field_schema::FieldSchema;

// TODO: Error on reserved field names - `insert`, etc

pub struct Field {
    pub ident: Ident,
    pub closure: syn::ExprClosure,
    pub schema: FieldSchema,
}

impl Parse for Field {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident = input.parse::<Ident>()?;
        let _ = input.parse::<syn::Token![:]>()?;
        let closure = input.parse::<syn::ExprClosure>()?;

        if closure.movability.is_some() {
            return Err(syn::Error::new_spanned(
                closure,
                "movable closures are not supported by besu!",
            ));
        }

        Ok(Self {
            ident,
            schema: FieldSchema::new(&closure.body),
            closure,
        })
    }
}
