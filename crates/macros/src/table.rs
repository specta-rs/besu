use itertools::multiunzip;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{spanned::Spanned, DeriveInput, Error, Result};

use crate::CRATE_NAME;

pub fn table(input: DeriveInput) -> Result<TokenStream> {
    let struct_ident = &input.ident;
    let mod_name = format_ident!("__gen_{struct_ident}");
    let model_struct_ident = format_ident!("{}Model", struct_ident);

    let fields = match input.data {
        syn::Data::Struct(ref data) => &data.fields,
        syn::Data::Enum(e) => {
            return Err(Error::new(
                e.enum_token.span(),
                "besu: A table must be defined as struct!",
            ))
        }
        syn::Data::Union(u) => {
            return Err(Error::new(
                u.union_token.span(),
                "besu: A table must be defined as struct!",
            ))
        }
    };

    let (model_structs, model_struct_fields_decls, model_struct_fields_impls, from_value_impl): (
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
    ) = multiunzip(fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;

        (
            quote! {
                pub struct #ident;

                #[automatically_derived]
                impl #CRATE_NAME::Field<#model_struct_ident> for #ident {
                    type Type = #ty;
                }
            },
            quote!(pub #ident: #ident),
            quote!(#ident),
            quote!(#ident: <#ty as #CRATE_NAME::FromValue>::from_value(vec![values.next().expect("length mismatch. skill issue.")])?),
        )
    }));

    // TODO: Emit something that doesn't have clippy warnings
    Ok(quote! {
        mod #mod_name {
            #[automatically_derived]
            impl #CRATE_NAME::FromValue for super::#struct_ident {
                fn from_value(values: Vec<#CRATE_NAME::SqlValue>) -> Result<Self, #CRATE_NAME::Error>
                where
                    Self: Sized
                {
                    let mut values = values.into_iter();
                    Ok(Self {
                       #( #from_value_impl ),*
                    })
                }
            }

            #( #model_structs )*

            pub struct #model_struct_ident {
                #( #model_struct_fields_decls ),*
            }

            #[automatically_derived]
            impl #CRATE_NAME::Model for #model_struct_ident {
                const NAME: &'static str = stringify!(#struct_ident); // TODO: Allow setting a custom name

                type Model = super::#struct_ident;
            }

            pub const #struct_ident: #model_struct_ident = #model_struct_ident {
                #( #model_struct_fields_impls ),*
            };
        }

        pub use #mod_name::#struct_ident;

    }
    .into())
}
