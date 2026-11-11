use heck::ToSnakeCase;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, Fields, Ident, Lit, LitStr, Result, Type,
    spanned::Spanned,
};

/// Resolve the path to the `besu` crate, so generated code keeps working when the
/// dependency is renamed.
fn besu_path() -> TokenStream {
    match crate_name("besu") {
        Ok(FoundCrate::Itself) => quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = Ident::new(&name, Span::call_site());
            quote!(::#ident)
        }
        Err(_) => quote!(::besu),
    }
}

pub fn expand(input: DeriveInput) -> Result<TokenStream> {
    let besu = besu_path();
    let struct_ident = input.ident.clone();

    if !input.generics.params.is_empty() {
        return Err(Error::new(
            input.generics.span(),
            "besu: tables cannot have generic parameters, because a table's row type must \
             be known at compile time",
        ));
    }

    let table_attrs = TableAttrs::parse(&input.attrs)?;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            Fields::Unit => {
                return Err(Error::new(
                    input.ident.span(),
                    "besu: a table needs at least one column",
                ));
            }
            Fields::Unnamed(unnamed) => {
                return Err(Error::new(
                    unnamed.span(),
                    "besu: a table must use named fields, because column names come from \
                     field names",
                ));
            }
        },
        Data::Enum(data) => {
            return Err(Error::new(
                data.enum_token.span(),
                "besu: a table must be a struct, not an enum",
            ));
        }
        Data::Union(data) => {
            return Err(Error::new(
                data.union_token.span(),
                "besu: a table must be a struct, not a union",
            ));
        }
    };

    let table_name = table_attrs.name.clone().unwrap_or_else(|| {
        LitStr::new(
            &struct_ident.to_string().to_snake_case(),
            struct_ident.span(),
        )
    });
    let handle_ident = Ident::new(
        &sanitise_ident(&table_name.value().to_snake_case()),
        struct_ident.span(),
    );
    let table_struct_ident = format_ident!("{}Table", struct_ident);
    let insert_struct_ident = format_ident!("{}Insert", struct_ident);
    let module_ident = format_ident!("__besu_table_{}", struct_ident.to_string().to_snake_case());

    let mut columns = Vec::new();
    let mut skipped = Vec::new();
    for field in fields {
        match Column::parse(field, &besu)? {
            Parsed::Column(column) => columns.push(column),
            Parsed::Skipped(ident) => skipped.push(ident),
        }
    }

    if columns.is_empty() {
        return Err(Error::new(
            struct_ident.span(),
            "besu: a table needs at least one column (every field was skipped)",
        ));
    }

    Ok(Codegen {
        besu,
        struct_ident,
        table_name,
        schema: table_attrs.schema,
        module_ident,
        handle_ident,
        table_struct_ident,
        insert_struct_ident,
        columns,
        skipped,
    }
    .finish())
}

fn sanitise_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if i == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("table");
    }
    out
}

struct TableAttrs {
    name: Option<LitStr>,
    schema: Option<LitStr>,
}

impl TableAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut this = Self {
            name: None,
            schema: None,
        };

        for attr in attrs {
            if !attr.path().is_ident("besu") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("table") || meta.path.is_ident("name") {
                    this.name = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("schema") {
                    this.schema = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                Err(meta.error("unrecognised table attribute; expected `table` or `schema`"))
            })?;
        }

        Ok(this)
    }
}

#[derive(Default)]
struct FieldAttrs {
    name: Option<LitStr>,
    primary_key: bool,
    auto_increment: bool,
    unique: bool,
    index: bool,
    skip: bool,
    default: Option<Default>,
    references: Option<LitStr>,
    on_delete: Option<String>,
    on_update: Option<String>,
}

impl FieldAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut this = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("besu") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("column") || meta.path.is_ident("name") {
                    this.name = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("primary_key") || meta.path.is_ident("primary") {
                    this.primary_key = true;
                    return Ok(());
                }
                if meta.path.is_ident("auto_increment") || meta.path.is_ident("autoincrement") {
                    this.auto_increment = true;
                    return Ok(());
                }
                if meta.path.is_ident("unique") {
                    this.unique = true;
                    return Ok(());
                }
                if meta.path.is_ident("index") {
                    this.index = true;
                    return Ok(());
                }
                if meta.path.is_ident("skip") {
                    this.skip = true;
                    return Ok(());
                }
                if meta.path.is_ident("default") {
                    let expr: Expr = meta.value()?.parse()?;
                    this.default = Some(Default::Literal(default_literal(&expr)?));
                    return Ok(());
                }
                if meta.path.is_ident("default_sql") {
                    let lit: LitStr = meta.value()?.parse()?;
                    this.default = Some(Default::Raw(lit));
                    return Ok(());
                }
                if meta.path.is_ident("default_now") {
                    this.default = Some(Default::Now);
                    return Ok(());
                }
                if meta.path.is_ident("references") {
                    this.references = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("on_delete") {
                    let lit: LitStr = meta.value()?.parse()?;
                    this.on_delete = Some(lit.value());
                    return Ok(());
                }
                if meta.path.is_ident("on_update") {
                    let lit: LitStr = meta.value()?.parse()?;
                    this.on_update = Some(lit.value());
                    return Ok(());
                }
                Err(meta.error(
                    "unrecognised column attribute; expected one of `column`, `primary_key`, \
                     `auto_increment`, `unique`, `index`, `skip`, `default`, `default_sql`, \
                     `default_now`, `references`, `on_delete`, `on_update`",
                ))
            })?;
        }

        Ok(this)
    }
}

enum Default {
    Literal(TokenStream),
    Raw(LitStr),
    Now,
}

fn default_literal(expr: &Expr) -> Result<TokenStream> {
    let Expr::Lit(lit) = expr else {
        return Err(Error::new(
            expr.span(),
            "besu: `default` must be a literal; use `default_sql` for an expression",
        ));
    };
    match &lit.lit {
        Lit::Str(value) => Ok(quote!(Text(#value))),
        Lit::Int(value) => Ok(quote!(Int(#value))),
        Lit::Float(value) => Ok(quote!(Float(#value))),
        Lit::Bool(value) => Ok(quote!(Bool(#value))),
        other => Err(Error::new(
            other.span(),
            "besu: `default` must be a string, integer, float or boolean literal",
        )),
    }
}

enum Parsed {
    Column(Column),
    Skipped(Ident),
}

struct Column {
    ident: Ident,
    sql_name: LitStr,
    /// The full field type, including any `Option`.
    full_ty: Type,
    /// The inner type, used for `Set<T>` and `Column<_, S, _>`.
    ty: Type,
    sql_ty: TokenStream,
    type_def: TokenStream,
    nullable: bool,
    primary_key: bool,
    auto_increment: bool,
    unique: bool,
    index: bool,
    default: Option<Default>,
    reference: Option<Reference>,
}

struct Reference {
    table: String,
    column: String,
    on_delete: Option<String>,
    on_update: Option<String>,
}

impl Column {
    fn parse(field: &syn::Field, besu: &TokenStream) -> Result<Parsed> {
        let attrs = FieldAttrs::parse(&field.attrs)?;
        let Some(ident) = field.ident.clone() else {
            return Err(Error::new(field.span(), "besu: expected a named field"));
        };

        if attrs.skip {
            return Ok(Parsed::Skipped(ident));
        }

        let sql_name = attrs
            .name
            .clone()
            .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));

        let (nullable, inner) = unwrap_option(&field.ty);
        let (sql_ty, type_def) = sql_type(inner, besu).map_err(|err| {
            Error::new(
                err.span(),
                format!(
                    "besu: `{}` is not a supported column type. Supported types are signed \
                     and unsigned integers, f32/f64, bool, String, Vec<u8>, uuid::Uuid, \
                     chrono::NaiveDate/NaiveTime/NaiveDateTime/DateTime<Utc> and \
                     serde_json::Value, each optionally wrapped in Option.",
                    quote!(#inner)
                ),
            )
        })?;

        if attrs.primary_key && nullable {
            return Err(Error::new(
                ident.span(),
                "besu: a primary key cannot be nullable",
            ));
        }

        if attrs.auto_increment && nullable {
            return Err(Error::new(
                ident.span(),
                "besu: an auto-incrementing column cannot be nullable",
            ));
        }

        let reference = match &attrs.references {
            Some(value) => {
                let text = value.value();
                let Some((table, column)) = text.split_once('.') else {
                    return Err(Error::new(
                        value.span(),
                        "besu: `references` must look like \"table.column\"",
                    ));
                };
                Some(Reference {
                    table: table.to_owned(),
                    column: column.to_owned(),
                    on_delete: attrs.on_delete.clone(),
                    on_update: attrs.on_update.clone(),
                })
            }
            None => None,
        };

        Ok(Parsed::Column(Column {
            ident,
            sql_name,
            full_ty: field.ty.clone(),
            ty: inner.clone(),
            sql_ty,
            type_def,
            nullable,
            primary_key: attrs.primary_key,
            auto_increment: attrs.auto_increment,
            unique: attrs.unique,
            index: attrs.index,
            default: attrs.default,
            reference,
        }))
    }
}

fn unwrap_option(ty: &Type) -> (bool, &Type) {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return (true, inner);
    }
    (false, ty)
}

fn sql_type(ty: &Type, besu: &TokenStream) -> Result<(TokenStream, TokenStream)> {
    let Type::Path(path) = ty else {
        return Err(Error::new(ty.span(), "unsupported"));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new(ty.span(), "unsupported"));
    };
    let name = segment.ident.to_string();

    let tokens = match name.as_str() {
        "i8" => (quote!(#besu::Int1), quote!(#besu::TypeDef::Int2)),
        "i16" => (quote!(#besu::Int2), quote!(#besu::TypeDef::Int2)),
        "i32" => (quote!(#besu::Int4), quote!(#besu::TypeDef::Int4)),
        "i64" => (quote!(#besu::Int8), quote!(#besu::TypeDef::Int8)),
        "u8" => (quote!(#besu::UInt1), quote!(#besu::TypeDef::UInt(8))),
        "u16" => (quote!(#besu::UInt2), quote!(#besu::TypeDef::UInt(16))),
        "u32" => (quote!(#besu::UInt4), quote!(#besu::TypeDef::UInt(32))),
        "u64" => (quote!(#besu::UInt8), quote!(#besu::TypeDef::UInt(64))),
        "f32" => (quote!(#besu::Float4), quote!(#besu::TypeDef::Float4)),
        "f64" => (quote!(#besu::Float8), quote!(#besu::TypeDef::Float8)),
        "bool" => (quote!(#besu::Bool), quote!(#besu::TypeDef::Bool)),
        "String" | "str" => (quote!(#besu::Text), quote!(#besu::TypeDef::Text)),
        "Uuid" => (quote!(#besu::Uuid), quote!(#besu::TypeDef::Uuid)),
        "DateTime" => (
            quote!(#besu::TimestampTz),
            quote!(#besu::TypeDef::TimestampTz),
        ),
        "NaiveDateTime" => (quote!(#besu::Timestamp), quote!(#besu::TypeDef::Timestamp)),
        "NaiveDate" => (quote!(#besu::Date), quote!(#besu::TypeDef::Date)),
        "NaiveTime" => (quote!(#besu::Time), quote!(#besu::TypeDef::Time)),
        "Value" => (quote!(#besu::Json), quote!(#besu::TypeDef::Json)),
        "Vec" => {
            let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
                return Err(Error::new(ty.span(), "unsupported"));
            };
            let Some(syn::GenericArgument::Type(Type::Path(inner))) = args.args.first() else {
                return Err(Error::new(ty.span(), "unsupported"));
            };
            if inner
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "u8")
            {
                (quote!(#besu::Bytes), quote!(#besu::TypeDef::Bytes))
            } else {
                return Err(Error::new(ty.span(), "unsupported"));
            }
        }
        _ => return Err(Error::new(ty.span(), "unsupported")),
    };

    Ok(tokens)
}

fn reference_action(besu: &TokenStream, value: Option<&str>) -> TokenStream {
    match value {
        None => quote!(#besu::ReferenceAction::Default),
        Some(value) => match value.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "cascade" => quote!(#besu::ReferenceAction::Cascade),
            "set_null" | "setnull" => quote!(#besu::ReferenceAction::SetNull),
            "set_default" | "setdefault" => quote!(#besu::ReferenceAction::SetDefault),
            "restrict" => quote!(#besu::ReferenceAction::Restrict),
            "no_action" | "noaction" => quote!(#besu::ReferenceAction::NoAction),
            _ => quote!(#besu::ReferenceAction::Default),
        },
    }
}

struct Codegen {
    besu: TokenStream,
    struct_ident: Ident,
    table_name: LitStr,
    schema: Option<LitStr>,
    module_ident: Ident,
    handle_ident: Ident,
    table_struct_ident: Ident,
    insert_struct_ident: Ident,
    columns: Vec<Column>,
    skipped: Vec<Ident>,
}

impl Codegen {
    fn finish(self) -> TokenStream {
        let besu = &self.besu;
        let model = &self.struct_ident;
        let module_ident = &self.module_ident;
        let handle_ident = &self.handle_ident;
        let table_struct_ident = &self.table_struct_ident;
        let insert_struct_ident = &self.insert_struct_ident;
        let table_name = &self.table_name;

        let schema = match &self.schema {
            Some(schema) => quote!(Some(#schema)),
            None => quote!(None),
        };

        let columns = &self.columns;

        // --- table handle fields ------------------------------------------------------
        let handle_fields = columns.iter().map(|column| {
            let ident = &column.ident;
            let sql_ty = &column.sql_ty;
            let nullability = if column.nullable {
                quote!(#besu::Nullable)
            } else {
                quote!(#besu::NonNull)
            };
            quote!(pub #ident: #besu::Column<#table_struct_ident, #sql_ty, #nullability>)
        });

        let handle_init = columns.iter().map(|column| {
            let ident = &column.ident;
            let name = &column.sql_name;
            quote!(#ident: #besu::Column::new(#name))
        });

        // --- static metadata ----------------------------------------------------------
        let column_defs = columns.iter().map(|column| {
            let name = &column.sql_name;
            let type_def = &column.type_def;
            let nullable = column.nullable;
            let auto_increment = column.auto_increment;
            let primary_key = column.primary_key;
            let unique = column.unique;
            let default = match &column.default {
                Some(Default::Literal(literal)) => {
                    quote!(Some(#besu::DefaultValue::#literal))
                }
                Some(Default::Raw(literal)) => quote!(Some(#besu::DefaultValue::Raw(#literal))),
                Some(Default::Now) => quote!(Some(#besu::DefaultValue::Now)),
                None => quote!(None),
            };
            quote! {
                #besu::ColumnDef {
                    name: #name,
                    ty: #type_def,
                    nullable: #nullable,
                    auto_increment: #auto_increment,
                    primary_key: #primary_key,
                    unique: #unique,
                    default: #default,
                }
            }
        });

        let index_defs = columns.iter().filter(|column| column.index).map(|column| {
            let name = LitStr::new(
                &format!(
                    "{}_{}_idx",
                    self.table_name.value(),
                    column.sql_name.value()
                ),
                column.ident.span(),
            );
            let column_name = &column.sql_name;
            quote! {
                #besu::Index {
                    name: #name,
                    columns: &[#column_name],
                    unique: false,
                }
            }
        });

        let foreign_keys = columns.iter().filter_map(|column| {
            let reference = column.reference.as_ref()?;
            let name = LitStr::new(
                &format!(
                    "{}_{}_fkey",
                    self.table_name.value(),
                    column.sql_name.value()
                ),
                column.ident.span(),
            );
            let column_name = &column.sql_name;
            let referenced_table = LitStr::new(&reference.table, column.ident.span());
            let referenced_column = LitStr::new(&reference.column, column.ident.span());
            let on_delete = reference_action(besu, reference.on_delete.as_deref());
            let on_update = reference_action(besu, reference.on_update.as_deref());
            Some(quote! {
                #besu::ForeignKey {
                    name: #name,
                    columns: &[#column_name],
                    references_table: #referenced_table,
                    references_columns: &[#referenced_column],
                    on_delete: #on_delete,
                    on_update: #on_update,
                }
            })
        });

        // --- decoding -----------------------------------------------------------------
        let decode_fields = columns.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.full_ty;
            quote!(#ident: <#ty as #besu::Decode<Dr>>::decode(row, offset)?)
        });
        let skipped_fields = self
            .skipped
            .iter()
            .map(|ident| quote!(#ident: ::core::default::Default::default()));
        let decode_bounds = columns.iter().map(|column| {
            let ty = &column.full_ty;
            quote!(#ty: #besu::Decode<Dr>)
        });
        let column_count = columns.len();

        // --- insert struct ------------------------------------------------------------
        let insert_fields = columns.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.ty;
            quote!(pub #ident: #besu::Set<#ty>)
        });

        let required: Vec<&Column> = columns
            .iter()
            .filter(|column| !column.nullable && !column.auto_increment && column.default.is_none())
            .collect();

        let required_args = required.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.ty;
            quote!(#ident: #ty)
        });

        let insert_initialisers = columns.iter().map(|column| {
            let ident = &column.ident;
            if required
                .iter()
                .any(|required| required.ident == column.ident)
            {
                quote!(#ident: #besu::Set::Value(#ident))
            } else {
                quote!(#ident: #besu::Set::Unset)
            }
        });

        let setters = columns.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.ty;
            let doc = format!("Set `{}`.", column.sql_name.value());
            quote! {
                #[doc = #doc]
                pub fn #ident(
                    mut self,
                    value: impl ::core::convert::Into<#besu::Set<#ty>>,
                ) -> Self {
                    self.#ident = value.into();
                    self
                }
            }
        });

        let column_names: Vec<_> = columns.iter().map(|column| &column.sql_name).collect();
        let insert_columns = columns.iter().map(|column| &column.sql_name);
        let insert_values = columns.iter().map(|column| {
            let ident = &column.ident;
            quote!(self.#ident.to_value())
        });

        let from_model = columns.iter().map(|column| {
            let ident = &column.ident;
            if column.auto_increment {
                quote!(#ident: #besu::Set::Unset)
            } else if column.nullable {
                quote!(#ident: model.#ident.into())
            } else {
                quote!(#ident: #besu::Set::Value(model.#ident))
            }
        });

        let model_doc = format!(
            "Insert a new row into the `{}` table.",
            self.table_name.value()
        );

        quote! {
            #[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
            mod #module_ident {
                use super::*;

                #[derive(Debug, Clone, Copy, PartialEq, Eq)]
                pub struct #table_struct_ident {
                    #(#handle_fields,)*
                }

                impl #table_struct_ident {
                    #[doc = #model_doc]
                    pub fn insert(&self, #(#required_args,)*) -> #insert_struct_ident {
                        #insert_struct_ident {
                            #(#insert_initialisers,)*
                        }
                    }
                }

                impl<RowSet, Path> #besu::Selection<RowSet, Path> for #table_struct_ident
                where
                    RowSet: #besu::Contains<Path, #table_struct_ident>,
                {
                    type Out = super::#model;

                    fn into_projection(
                        self,
                    ) -> (
                        ::std::vec::Vec<#besu::ExprKind>,
                        usize,
                    ) {
                        let mut exprs = ::std::vec::Vec::new();
                        let mut columns = 0;
                        #(
                            exprs.push(#besu::ExprKind::Column {
                                table: #table_name,
                                column: #column_names,
                            });
                            columns += 1;
                        )*
                        (exprs, columns)
                    }
                }

                impl #besu::Table for #table_struct_ident {
                    type Model = super::#model;
                    type Insert = #insert_struct_ident;

                    const DEF: &'static #besu::TableDef = &#besu::TableDef {
                        name: #table_name,
                        schema: #schema,
                        columns: &[#(#column_defs),*],
                        indexes: &[#(#index_defs),*],
                        foreign_keys: &[#(#foreign_keys),*],
                    };
                }

                /// The table handle for this table.
                pub const #handle_ident: #table_struct_ident = #table_struct_ident {
                    #(#handle_init,)*
                };

                /// A new row to insert.
                #[derive(Debug, Clone, Default)]
                pub struct #insert_struct_ident {
                    #(#insert_fields,)*
                }

                impl #insert_struct_ident {
                    #(#setters)*
                }

                impl #besu::Insertable for #insert_struct_ident {
                    const COLUMNS: &'static [&'static str] = &[#(#insert_columns),*];

                    fn into_values(self) -> ::std::vec::Vec<::core::option::Option<#besu::Value>> {
                        ::std::vec![#(#insert_values),*]
                    }
                }

                impl ::core::convert::From<super::#model> for #insert_struct_ident {
                    fn from(model: super::#model) -> Self {
                        Self {
                            #(#from_model,)*
                        }
                    }
                }

                impl<Dr: #besu::Driver> #besu::Decode<Dr> for super::#model
                where
                    #(#decode_bounds,)*
                {
                    const COLUMNS: usize = #column_count;

                    fn decode(
                        row: &Dr::Row,
                        offset: &mut usize,
                    ) -> ::core::result::Result<Self, Dr::Error> {
                        Ok(Self {
                            #(#decode_fields,)*
                            #(#skipped_fields,)*
                        })
                    }
                }
            }

            pub use #module_ident::{#handle_ident, #insert_struct_ident, #table_struct_ident};
        }
    }
}
