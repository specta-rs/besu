use std::collections::HashSet;

use itertools::multiunzip;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{spanned::Spanned, DeriveInput, Error, Result};

use crate::CRATE_NAME;

pub fn table(input: DeriveInput) -> Result<TokenStream> {
    let struct_ident = &input.ident;
    let mod_name = format_ident!("__besu_table_gen_{struct_ident}");
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

    let (
        model_structs,
        model_struct_fields_decls,
        model_struct_fields_impls,
        mut from_value_impl,
        field_types,
    ): (Vec<_>, Vec<_>, Vec<_>, Vec<_>, HashSet<_>) = multiunzip(fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;

        (
            quote! {
                pub struct #ident;

                #[automatically_derived]
                impl #CRATE_NAME::Column<#model_struct_ident> for #ident {
                    const NAME: &'static str = stringify!(#ident); // TODO: Allow setting a custom name, lowercase by default

                    type Type = #ty;
                }
            },
            quote!(pub #ident: #ident),
            quote!(#ident),
            quote!(#ident: {
                let v = D::get_value(&row, i);
                if i != 0 { i -= 1 };
                <D::ValueDecoder as #CRATE_NAME::DecodeValue<D, #ty>>::decode(v)
            }),
            ty.to_token_stream().to_string(),
        )
    }));
    from_value_impl.reverse();

    let field_types = field_types.iter().map(|s| {
        let ty: TokenStream = s.parse().unwrap();
        quote!(<D as #CRATE_NAME::Driver>::ValueDecoder: #CRATE_NAME::DecodeValue<D, #ty>)
    });

    // TODO: Emit something that doesn't have clippy warnings
    Ok(quote! {
        mod #mod_name {
            #[automatically_derived]
            impl #CRATE_NAME::Model for super::#struct_ident {}

            #[automatically_derived]
            impl<D: #CRATE_NAME::Driver> #CRATE_NAME::DecodeRow<D> for super::#struct_ident
            where
                #( #field_types ),*
            {
                fn decode(row: D::Row) -> Self {
                    let mut i = D::row_len(&row);
                    if i != 0 { i -= 1 };
                    Self {
                        #( #from_value_impl ),*
                    }
                }
            }

            #( #model_structs )*

            pub struct #model_struct_ident {
                #( #model_struct_fields_decls ),*
            }

            #[automatically_derived]
            impl #CRATE_NAME::Table for #model_struct_ident {
                const NAME: &'static str = stringify!(#struct_ident); // TODO: Allow setting a custom name, lowercase by default

                type Model = super::#struct_ident;
            }

            pub const #struct_ident: #model_struct_ident = #model_struct_ident {
                #( #model_struct_fields_impls ),*
            };
        }

        pub use #mod_name::#struct_ident;

    })
}
