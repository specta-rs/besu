use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Error, Fields, LitStr, Result, spanned::Spanned};

use crate::table::besu_path;

pub fn expand(input: DeriveInput) -> Result<TokenStream> {
    let besu = besu_path();
    let enum_ident = input.ident.clone();

    if !input.generics.params.is_empty() {
        return Err(Error::new(
            input.generics.span(),
            "besu: a `SqlEnum` cannot have generic parameters, because the labels are a \
             compile-time constant",
        ));
    }

    let attrs = EnumAttrs::parse(&input.attrs)?;

    let variants = match &input.data {
        Data::Enum(data) => &data.variants,
        Data::Struct(data) => {
            return Err(Error::new(
                data.struct_token.span(),
                "besu: `SqlEnum` must be derived for an enum, not a struct",
            ));
        }
        Data::Union(data) => {
            return Err(Error::new(
                data.union_token.span(),
                "besu: `SqlEnum` must be derived for an enum, not a union",
            ));
        }
    };

    if variants.is_empty() {
        return Err(Error::new(
            enum_ident.span(),
            "besu: a `SqlEnum` needs at least one variant, because an enum with no labels \
             allows no value",
        ));
    }

    let name = attrs
        .name
        .unwrap_or_else(|| LitStr::new(&enum_ident.to_string().to_snake_case(), enum_ident.span()));

    let mut labelled: Vec<(syn::Ident, LitStr)> = Vec::with_capacity(variants.len());
    for variant in variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new(
                variant.fields.span(),
                "besu: a `SqlEnum` variant cannot hold data; only a unit variant is a label",
            ));
        }
        let rename = VariantAttrs::parse(&variant.attrs)?;
        let label = match rename {
            Some(label) => label,
            None => LitStr::new(
                &rename_all(attrs.rename_all, &variant_ident_str(&variant.ident)),
                variant.ident.span(),
            ),
        };
        if let Some((previous, _)) = labelled
            .iter()
            .find(|(_, existing)| existing.value() == label.value())
        {
            return Err(Error::new(
                label.span(),
                format!(
                    "besu: `{}` and `{}` are both the label `{}`; every label must be unique",
                    previous,
                    variant.ident,
                    label.value()
                ),
            ));
        }
        labelled.push((variant.ident.clone(), label));
    }

    let labels: Vec<&LitStr> = labelled.iter().map(|(_, label)| label).collect();
    let to_label = labelled
        .iter()
        .map(|(variant, label)| quote!(#enum_ident::#variant => #label));
    let from_label = labelled.iter().map(
        |(variant, label)| quote!(#label => ::core::result::Result::Ok(#enum_ident::#variant)),
    );

    Ok(quote! {
        impl #besu::SqlEnum for #enum_ident {
            const DEF: #besu::besu_core::types::EnumDef =
                #besu::besu_core::types::EnumDef::new(#name, &[#(#labels),*]);

            const LABELS: &'static [&'static str] = &[#(#labels),*];

            fn label(&self) -> &'static str {
                match self {
                    #(#to_label,)*
                }
            }

            fn from_label(label: &str) -> ::core::result::Result<Self, #besu::UnknownLabel> {
                match label {
                    #(#from_label,)*
                    other => ::core::result::Result::Err(#besu::UnknownLabel::new(other)),
                }
            }
        }

        impl #besu::SqlType for #enum_ident {
            const DEF: #besu::TypeDef =
                #besu::TypeDef::Enum(<#enum_ident as #besu::SqlEnum>::DEF);
            type Rust = Self;
        }

        impl ::core::convert::From<#enum_ident> for #besu::Value {
            fn from(value: #enum_ident) -> Self {
                #besu::Value::EnumLabel(::std::string::String::from(
                    <#enum_ident as #besu::SqlEnum>::label(&value),
                ))
            }
        }

        impl #besu::expr::IntoSqlValue<#enum_ident> for #enum_ident {
            fn into_sql_value(self) -> #besu::Value {
                ::core::convert::From::from(self)
            }
        }

        impl<__BesuDr: #besu::Driver> #besu::Decode<__BesuDr> for #enum_ident {
            const COLUMNS: usize = 1;

            fn decode(
                row: &<__BesuDr as #besu::Driver>::Row,
                offset: &mut usize,
            ) -> ::core::result::Result<Self, <__BesuDr as #besu::Driver>::Error> {
                let index = *offset;
                let label = <__BesuDr as #besu::Driver>::decode_text(row, index)?;
                *offset += 1;
                <#enum_ident as #besu::SqlEnum>::from_label(&label).map_err(|err| {
                    <__BesuDr as #besu::Driver>::error(#besu::Error::decode(
                        index,
                        ::std::string::ToString::to_string(&err),
                    ))
                })
            }
        }
    })
}

/// The identifier a label is derived from, without the `r#` of a raw identifier.
fn variant_ident_str(ident: &syn::Ident) -> String {
    let ident = ident.to_string();
    match ident.strip_prefix("r#") {
        Some(ident) => ident.to_owned(),
        None => ident,
    }
}

/// How a variant's name is turned into a label.
#[derive(Clone, Copy)]
enum RenameAll {
    SnakeCase,
    CamelCase,
    PascalCase,
    ScreamingSnakeCase,
    Lowercase,
    Uppercase,
    KebabCase,
}

fn parse_rename_all(value: &LitStr) -> Result<RenameAll> {
    Ok(match value.value().as_str() {
        "snake_case" => RenameAll::SnakeCase,
        "camelCase" => RenameAll::CamelCase,
        "PascalCase" => RenameAll::PascalCase,
        "SCREAMING_SNAKE_CASE" => RenameAll::ScreamingSnakeCase,
        "lowercase" => RenameAll::Lowercase,
        "UPPERCASE" => RenameAll::Uppercase,
        "kebab-case" => RenameAll::KebabCase,
        other => {
            return Err(Error::new(
                value.span(),
                format!(
                    "besu: `rename_all = \"{other}\"` is not a rename rule; expected \
                     `snake_case`, `camelCase`, `PascalCase`, `SCREAMING_SNAKE_CASE`, \
                     `lowercase`, `UPPERCASE` or `kebab-case`"
                ),
            ));
        }
    })
}

fn rename_all(rule: Option<RenameAll>, name: &str) -> String {
    match rule {
        // Without a rule the variant's name is the label, which is what serde does.
        None => name.to_owned(),
        Some(RenameAll::SnakeCase) => name.to_snake_case(),
        Some(RenameAll::CamelCase) => name.to_lower_camel_case(),
        // A variant is already written in PascalCase, so the rule only says not to lowercase
        // it.
        Some(RenameAll::PascalCase) => name.to_owned(),
        Some(RenameAll::ScreamingSnakeCase) => name.to_shouty_snake_case(),
        Some(RenameAll::Lowercase) => name.to_lowercase(),
        Some(RenameAll::Uppercase) => name.to_uppercase(),
        Some(RenameAll::KebabCase) => name.to_kebab_case(),
    }
}

struct EnumAttrs {
    name: Option<LitStr>,
    rename_all: Option<RenameAll>,
}

impl EnumAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut this = Self {
            name: None,
            rename_all: None,
        };

        for attr in attrs {
            if !attr.path().is_ident("besu") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    this.name = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("rename_all") {
                    let value: LitStr = meta.value()?.parse()?;
                    this.rename_all = Some(parse_rename_all(&value)?);
                    return Ok(());
                }
                Err(meta.error(
                    "unrecognised `SqlEnum` attribute; expected `name = \"...\"` or \
                     `rename_all = \"...\"`",
                ))
            })?;
        }

        Ok(this)
    }
}

struct VariantAttrs;

impl VariantAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Option<LitStr>> {
        let mut rename = None;

        for attr in attrs {
            if !attr.path().is_ident("besu") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    rename = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                Err(meta.error("unrecognised variant attribute; expected `rename = \"...\"`"))
            })?;
        }

        Ok(rename)
    }
}
