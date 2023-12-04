use proc_macro2::{Ident, TokenStream};
use quote::{quote, ToTokens};
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    token::Comma,
    Token,
};

use crate::{table::field_schema::FieldType, CRATE_NAME};

use super::field::Field;

pub struct Table {
    pub struct_name: Ident,
    pub scope_name: Ident,
    pub fields: Punctuated<Field, Comma>,
}

impl Parse for Table {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let model_name = input.parse::<Ident>()?;
        let _ = input.parse::<syn::Token![,]>()?;
        let module_name = input.parse::<Ident>()?;
        let _ = input.parse::<syn::Token![,]>()?;
        let content;
        syn::braced!(content in input);

        Ok(Self {
            struct_name: model_name,
            scope_name: module_name,
            fields: content.parse_terminated(Field::parse, Token![,])?,
        })
    }
}

impl Table {
    pub fn model_struct(&self) -> TokenStream {
        #[allow(unused_mut)]
        let mut extra_derive: Vec<TokenStream> = vec![];

        #[cfg(feature = "serde")]
        {
            extra_derive.push(quote!(serde::Serialize));
            extra_derive.push(quote!(serde::Deserialize));
        }

        #[cfg(feature = "specta")]
        extra_derive.push(quote!(specta::Type));

        let (fields_decl, fields_value_impl): (Vec<_>, Vec<_>) = self
            .fields
            .iter()
            .map(|f| {
                let ident = &f.ident;
                let ty = &f.schema.rust_ty();

                (quote!(pub #ident: #ty), quote!(#ident: <#ty as #CRATE_NAME::FromValue>::from_value(vec![values.next().expect("length mismatch. skill issue.")])?))
            })
            .unzip();

        let model_name = &self.struct_name;
        quote! {
            #[derive(Debug, Clone #(, #extra_derive)*)]
            pub struct #model_name {
                #(#fields_decl),*
            }

            impl #CRATE_NAME::schema::Model for #model_name {}

            impl #CRATE_NAME::FromValue for #model_name {
                fn from_value(values: Vec<#CRATE_NAME::SqlValue>) -> Result<Self, #CRATE_NAME::Error>
                where
                    Self: Sized
                {
                    let mut values = values.into_iter();
                    Ok(Self {
                        #(#fields_value_impl),*
                    })
                }
            }
        }
    }

    pub fn model_module_model_struct(&self) -> TokenStream {
        let struct_name = &self.struct_name;
        let scope_name = &self.scope_name.to_string();

        let (field_names, field_types): (Vec<_>, Vec<_>) = self
            .fields
            .iter()
            .map(|f| {
                let ident = &f.ident.to_string();
                let ty = f.schema.rust_ty().to_string();

                (quote!(#ident), quote!(#ty))
            })
            .unzip();

        quote! {
            #[doc(hidden)]
            pub struct model;

            impl #CRATE_NAME::schema::Table for model {
                type Model = super::#struct_name;

                fn def() -> #CRATE_NAME::schema::TableDefinition {
                    #CRATE_NAME::internal::construct::table_def(
                        #scope_name,
                        &[#(#field_names),*],
                        &[#(#field_types),*] // TODO: Plz move this from runtime string comparisons to macro string comparisions
                    )
                }
            }
        }
    }

    pub fn model_module_fields(&self) -> TokenStream {
        let struct_name = &self.struct_name;
        let scope_name = &self.scope_name.to_string();
        let fields = self.fields.iter().map(|f| {
            let ident = &f.ident;
            let column_name = ident.to_string();
            let column_type_rs = f.schema.rust_ty();

            quote!(
                pub struct #ident;

                impl #CRATE_NAME::schema::Column for #ident {
                    type Model = super::#struct_name;
                    type Type = #column_type_rs;

                    fn def() -> #CRATE_NAME::schema::ColumnDefinition {
                        #CRATE_NAME::internal::construct::column_def(#scope_name, #column_name)
                    }
                }
            )
        });

        quote!(#(#fields)*)
    }

    pub fn model_module(&self) -> TokenStream {
        let scope_name = &self.scope_name;
        let lsp_hints = self.lsp_hints();
        let model = self.model_module_model_struct();
        let fields = self.model_module_fields();
        let insert_struct = self.insert_struct();

        quote! {
            // TODO: Doc hidden on this?
            pub const fn #scope_name() -> #scope_name::model {
                #scope_name::model
            }

            #[allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
            pub mod #scope_name {
                mod lsp_hints {
                    #lsp_hints
                }

                #model

                #fields

                #insert_struct
            }
        }
    }

    pub fn insert_struct(&self) -> TokenStream {
        // TODO: Any field with a default should be optional

        let scope_name_str = self.scope_name.to_string();

        let fields = self.fields.iter().map(|f| {
            let ident = &f.ident;
            let mut ty = f.schema.rust_ty();
            if (f.schema.ty == FieldType::Serial && !f.schema.optional)
                || f.schema.default.is_some()
            {
                ty = quote!(Option<#ty>);
            }

            quote!(pub #ident: #ty)
        });

        let field_methods = self.fields.iter().map(|f| {
            let ident = &f.ident;
            let ty = f.schema.rust_ty();

            let mut value = quote!(#ident.into());
            if f.schema.ty == FieldType::Serial || f.schema.default.is_some() {
                value = quote!(Some(#value));
            }

            quote!(pub fn #ident<V: Into<#ty>>(mut self, #ident: V) -> Self {
                self.#ident = #value;
                self
            })
        });

        let insert_field_methods = self.fields.iter().map(|f| {
            let ident = &f.ident;
            let ty = f.schema.rust_ty();

            let mut value = quote!(#ident.into());
            if f.schema.ty == FieldType::Serial || f.schema.default.is_some() {
                value = quote!(Some(#value));
            }

            quote!(pub fn #ident<V: Into<#ty>>(&mut self, #ident: V) -> &mut Self {
                self.0.#ident = #value;
                self
            })
        });

        let insert_fn_args = self
            .fields
            .iter()
            .filter(|f| {
                f.schema.ty != FieldType::Serial && !f.schema.optional && f.schema.default.is_none()
            })
            .map(|f| {
                let ident = &f.ident;
                let ty = f.schema.rust_ty();

                quote!(#ident: impl Into<#ty>)
            });

        let insert_fn_values = self.fields.iter().map(|f| {
            let ident = &f.ident;

            if f.schema.optional || f.schema.ty == FieldType::Serial || f.schema.default.is_some() {
                quote!(#ident: None)
            } else {
                quote!(#ident: #ident.into())
            }
        });

        let field_names_str = self.fields.iter().map(|f| {
            let ident = f.ident.to_string();
            quote!(#ident)
        });

        let field_values = self.fields.iter().map(|f| {
            let ident = &f.ident;

            if let Some(default) = &f.schema.default {
                // TODO: If the database supports `DEFAULT`
                // quote!(self.#ident.map(|v| v.into()).unwrap_or(#CRATE_NAME::SqlValue::Default))

                // Workaround for SQLite not supporting `DEFAULT`
                quote!(self.#ident.map(|v| v.into()).unwrap_or_else(|| #default))
            } else {
                quote!(self.#ident.into())
            }
        });

        quote! {
            // We need a new scope so that the argument name doesn't pick up as the `const`.
            mod internal {
                #[derive(Debug, Default)] // TODO: Remove this `Default` impl once the macros are smarter
                pub struct insert {
                    #(#fields),*
                }

                pub fn insert(#(#insert_fn_args),*) -> insert {
                    insert {
                        #(#insert_fn_values),*
                    }
                }

                impl #CRATE_NAME::queries::InsertValue for insert {
                    fn def() -> #CRATE_NAME::queries::InsertDefinition {
                        #CRATE_NAME::internal::construct::insert_def(#scope_name_str, &[#(#field_names_str),*])
                    }

                    fn values(self) -> Vec<#CRATE_NAME::SqlValue> {
                        vec![#(#field_values),*]
                    }
                }

                impl insert {
                    pub fn edit(&mut self) -> insert_mut {
                        insert_mut(self)
                    }

                    #(#field_methods)*
                }

                pub struct insert_mut<'a>(&'a mut insert);

                impl<'a> insert_mut<'a> {
                    #(#insert_field_methods)*
                }
            }
            pub use internal::{insert, insert_mut};


        }
    }

    pub fn lsp_hints(&self) -> TokenStream {
        let fields = self.fields.iter().map(|f| {
            let ident = &f.ident;
            let closure = &f.closure;
            quote!(
                #[allow(unused)]
                fn #ident() -> impl Fn(#CRATE_NAME::schema::SchemaBuilder) -> #CRATE_NAME::schema::SchemaBuilder {
                    #closure
                }
            )
        });

        quote! {
            mod _lsp_hints {
                #(#fields)*
            }
        }
    }
}

impl ToTokens for Table {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let s = self.model_struct();
        let m = self.model_module();
        tokens.extend(quote! {
            #s
            #m
        })
    }
}
