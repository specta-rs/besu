use std::collections::HashSet;

use itertools::multiunzip;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{spanned::Spanned, Attribute, DeriveInput, Error, LitStr, Result};

use crate::CRATE_NAME;

pub fn table(input: DeriveInput) -> Result<TokenStream> {
    let struct_ident = &input.ident;
    let struct_attrs = TableAttr::parse_from(&input.attrs)?;

    let mod_name = format_ident!("__besu_table_gen_{struct_ident}");
    let model_struct_ident = format_ident!("{}Model", struct_ident);
    let insert_struct_ident = format_ident!("{}Insert", struct_ident);
    let insert_struct_ident_public = format_ident!("{}Insert", struct_ident); // TODO: Configurable

    // TODO: Casing + Apply schema bit
    let sql_table_name = struct_attrs
        .name
        .unwrap_or_else(|| LitStr::new(&struct_ident.to_string(), struct_ident.span()));

    if !input.generics.params.is_empty() {
        return Err(Error::new(
            input.generics.params.span(),
            "besu: Tables cannot have generic parameters!",
        ));
    }

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

    let fields = fields
        .iter()
        .map(|f| Ok((f, FieldAttr::parse_from(&f.attrs)?)))
        .collect::<Result<Vec<_>>>()?;

    let field_types: HashSet<_> = fields
        .iter()
        .map(|(f, _)| f.ty.to_token_stream().to_string())
        .collect();

    let (
        field_structs,
        field_names,
        model_struct_fields_decls,
        model_struct_fields_impls,
        mut decode_row_impl,
        insert_row_impl,
        insert_struct_fields_decls,
        required_fields_fn_args,
        required_fields_fn_impl,
        insert_from_model_struct_fields_impls,
        insert_struct_setter,
        macro_field_match,
    ): (
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
    ) = multiunzip(fields.iter().enumerate().map(|(i, (f, field_attrs))| {
        // let ident = &f.ident;
        let Some(ident) = &f.ident else {
            todo!();
        };

        let ty = &f.ty;
        let mut ty_str = f.ty.to_token_stream().to_string();
        remove_whitespace(&mut ty_str);

        // TODO: Casing
        let sql_table_name = field_attrs
            .name
            .clone()
            .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));

        // It would be nice if this could be runtime but I don't think it can be.
        let is_optional = field_attrs.primary_key || ty_str.starts_with("Option<"); // TODO: Or `std::option::Option`

        let setter_impl = if !is_optional {
            quote!(value)
        } else {
            quote!(Some(value))
        };

        let from_model_to_insert_impl = if is_optional {
            quote!(Some(model.#ident))
        } else {
            quote!(model.#ident)
        };

        let insert_decode_impl = if is_optional {
            quote!(self.#ident.clone().expect("todo: optional")) // TODO: Dealing with this `Clone` bound
        } else {
            quote!(self.#ident.clone()) // TODO: Dealing with this `Clone` bound
        };

        (
            quote! {
                pub struct #ident;

                #[automatically_derived]
                impl #CRATE_NAME::Column<super::#model_struct_ident> for #ident {
                    const NAME: &'static str = #sql_table_name;

                    type Type = #ty;
                }
            },
            quote!(#ident),
            quote!(pub #ident: fields::#ident),
            quote!(#ident: fields::#ident),
            quote!(#ident: {
                let v = D::get_value(&row, i)?;
                if i != 0 { i -= 1 };
                <D::ValueDecoder as #CRATE_NAME::DecodeValue<D, #ty>>::decode(v)?
            }),
            quote!(#i => <D as #CRATE_NAME::EncodeArgument<#ty>>::encode(arguments, #insert_decode_impl)),
            if is_optional {
                quote!(pub #ident: Option<#ty>)
            } else {
                quote!(pub #ident: #ty)
            },
            (!is_optional).then(|| quote!(#ident: #ty)),
            if is_optional {
                quote!(#ident: None)
            } else {
                quote!(#ident)
            },
            quote!(#ident: #from_model_to_insert_impl),
            quote! {
                pub fn #ident(mut self, value: #ty) -> Self {
                    self.#ident = #setter_impl;
                    self
                }
            },
            quote! {
                (@verify; #ident) => {};
                (@field; #ident) => { #ty };
            },
        )
    }));
    decode_row_impl.reverse();
    let required_fields_fn_args = required_fields_fn_args.into_iter().flatten();

    let decode_field_types = field_types.iter().map(|s| {
        let ty: TokenStream = s
            .parse()
            .expect("token stream -> string -> token stream failed!");
        quote!(<D as #CRATE_NAME::Driver>::ValueDecoder: #CRATE_NAME::DecodeValue<D, #ty>)
    });

    let insert_field_types = field_types.iter().map(|s| {
        let ty: TokenStream = s
            .parse()
            .expect("token stream -> string -> token stream failed!");
        quote!(D: for<'a> #CRATE_NAME::EncodeArgument<'a, #ty>)
    });

    // TODO: Emit something that doesn't have clippy warnings
    Ok(quote! {
        #[allow(non_upper_case_globals, non_camel_case_types, none_snake_case)]
        mod #mod_name {
            #[automatically_derived]
            impl<D: #CRATE_NAME::Driver> #CRATE_NAME::DecodeRow<D> for super::#struct_ident
            where
                #( #decode_field_types ),*
            {
                fn decode(row: D::Row) -> ::std::result::Result<Self, ::std::boxed::Box<dyn ::std::error::Error + Send + Sync + 'static>>
                where
                    Self: ::std::marker::Sized
                {
                    let mut i = D::row_len(&row);
                    if i != 0 { i -= 1 };
                    Ok(Self {
                        #( #decode_row_impl ),*
                    })
                }
            }

            // This scoped to ensure it doesn't break other stuff because `pub struct id;` & `fn todo(id: i32) {}` counts as a destructure which is a problem.
            mod fields {
                #( #field_structs )*
            }

            #[derive(Debug, Clone, Default)]
            pub struct #insert_struct_ident {
                #( #insert_struct_fields_decls ),*
            }

            impl From<super::#struct_ident> for #insert_struct_ident {
                fn from(model: super::#struct_ident) -> Self {
                    Self {
                        #( #insert_from_model_struct_fields_impls ),*
                    }
                }
            }

            impl #insert_struct_ident {
                #( #insert_struct_setter )*
            }

            pub struct #model_struct_ident {
                #( #model_struct_fields_decls ),*
            }

            impl #model_struct_ident {
                pub fn create(&self #(,#required_fields_fn_args)*) -> #insert_struct_ident {
                    #insert_struct_ident {
                        #( #required_fields_fn_impl ),*
                    }
                }
            }

            #[automatically_derived]
            impl #CRATE_NAME::Table for #model_struct_ident {
                type Model = super::#struct_ident;
                type Insert = #insert_struct_ident;

                fn name(&self) -> &'static str {
                    #sql_table_name
                }

                fn selection(&self) -> &'static [&'static str] {
                    use #CRATE_NAME::Column;
                    &[
                        #( <fields::#field_names as Column<_>>::NAME ),*
                    ]
                }


            }

            #[automatically_derived]
            impl<D: #CRATE_NAME::Driver> #CRATE_NAME::Insertable<D> for #insert_struct_ident
            where
                #( #insert_field_types ),*
            {
                fn columns() -> &'static [&'static str] {
                    use #CRATE_NAME::Column;
                    &[
                        #( <fields::#field_names as Column<_>>::NAME ),*
                    ]
                }

                fn encode<'a>(&self, arguments: &mut D::Arguments<'a>, field: usize) -> ::std::result::Result<(), ::std::boxed::Box<dyn ::std::error::Error + Send + Sync + 'static>> {
                    match field {
                        #( #insert_row_impl, )*
                        _ => todo!(), // TODO: Return error instead
                    }
                }
            }

            pub const #struct_ident: #model_struct_ident = #model_struct_ident {
                #( #model_struct_fields_impls ),*
            };
        }


        #[allow(non_upper_case_globals, non_camel_case_types, none_snake_case)]
        pub use #mod_name::#struct_ident;

        pub use #mod_name::#insert_struct_ident as #insert_struct_ident_public;

        #[allow(unused_macros)]
        macro_rules! #struct_ident {
            #( #macro_field_match )*
            (@verify; $i:ident) => {
                compile_error!(concat!("selected non-existent field '", stringify!($i), "' from '", stringify!(#struct_ident), "' using `besu::select`. Check your selection!"));
            };
            (@field; $i:ident) => { () };
        }
    })
}

struct TableAttr {
    name: Option<LitStr>,
    schema: Option<LitStr>,
    // TODO: Configuring indexes, compound primary keys
}

impl TableAttr {
    pub fn parse_from(attributes: &Vec<Attribute>) -> Result<Self> {
        let mut this = Self {
            name: None,
            schema: None,
        };

        for attr in attributes {
            if !attr.path().is_ident("table") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    this.name = Some(meta.value()?.parse()?);
                    return Ok(());
                }

                if meta.path.is_ident("schema") {
                    this.schema = Some(meta.value()?.parse()?);
                    return Ok(());
                }

                Err(meta.error("unrecognized attribute"))
            })?;
        }

        Ok(this)
    }
}

struct FieldAttr {
    name: Option<LitStr>,
    primary_key: bool,
    // TODO: override database type
    // TODO: default value (in Rust and DB)
}

impl FieldAttr {
    pub fn parse_from(attributes: &Vec<Attribute>) -> Result<Self> {
        let mut this = Self {
            name: None,
            primary_key: false,
        };

        for attr in attributes {
            if !attr.path().is_ident("column") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    this.name = Some(meta.value()?.parse()?);
                    return Ok(());
                }

                if meta.path.is_ident("primary_key") {
                    this.primary_key = true;
                    return Ok(());
                }

                Err(meta.error("unrecognized attribute"))
            })?;
        }

        Ok(this)
    }
}

fn remove_whitespace(s: &mut String) {
    s.retain(|c| !c.is_whitespace());
}
