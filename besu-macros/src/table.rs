use heck::ToSnakeCase;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, Fields, Ident, Lit, LitStr, Result, Token, Type,
    parse::Parse, spanned::Spanned,
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
            Parsed::Column(column) => columns.push(*column),
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
        indexes: table_attrs.indexes,
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
    indexes: Vec<IndexDecl>,
}

impl TableAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut this = Self {
            name: None,
            schema: None,
            indexes: Vec::new(),
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
                if meta.path.is_ident("index") {
                    let content;
                    syn::parenthesized!(content in meta.input);
                    this.indexes.push(IndexDecl::parse(&content)?);
                    return Ok(());
                }
                Err(meta
                    .error("unrecognised table attribute; expected `table`, `schema` or `index`"))
            })?;
        }

        Ok(this)
    }
}

/// What one derived index entry targets.
enum IndexTargetDecl {
    Column(String),
    Expression(String),
}

/// One entry in a table-level `#[besu(index(columns(...)))]`.
struct IndexEntryDecl {
    target: IndexTargetDecl,
    opclass: Option<LitStr>,
    span: Span,
}

/// A table-level `#[besu(index(...))]` declaration.
struct IndexDecl {
    entries: Vec<IndexEntryDecl>,
    name: Option<LitStr>,
    unique: bool,
    /// The `IndexKind` variant, when `using = "..."` names one.
    kind: Option<TokenStream>,
    bare_opclass: Option<LitStr>,
}

impl IndexDecl {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut this = Self {
            entries: Vec::new(),
            name: None,
            unique: false,
            kind: None,
            bare_opclass: None,
        };
        let mut opclasses: Vec<(Ident, LitStr)> = Vec::new();

        while !input.is_empty() {
            let keyword: Ident = input.parse()?;
            if keyword == "columns" {
                let content;
                syn::parenthesized!(content in input);
                let exprs = content.parse_terminated(Expr::parse, Token![,])?;
                if exprs.is_empty() {
                    return Err(Error::new(
                        keyword.span(),
                        "besu: `columns` needs at least one column or expression",
                    ));
                }
                this.entries = exprs
                    .into_iter()
                    .map(|expr| index_entry(&expr))
                    .collect::<Result<_>>()?;
            } else if keyword == "name" {
                input.parse::<Token![=]>()?;
                this.name = Some(input.parse()?);
            } else if keyword == "unique" {
                this.unique = true;
            } else if keyword == "using" {
                input.parse::<Token![=]>()?;
                let lit: LitStr = input.parse()?;
                this.kind = Some(index_kind(&lit)?);
            } else if keyword == "opclass" {
                // `opclass = "..."` names the class for the only entry; `opclass(a = "...")`
                // names one per column. The two spellings cannot be mixed.
                if input.peek(Token![=]) {
                    input.parse::<Token![=]>()?;
                    this.bare_opclass = Some(input.parse()?);
                } else {
                    let content;
                    syn::parenthesized!(content in input);
                    while !content.is_empty() {
                        let column: Ident = content.parse()?;
                        content.parse::<Token![=]>()?;
                        let value: LitStr = content.parse()?;
                        opclasses.push((column, value));
                        if content.peek(Token![,]) {
                            content.parse::<Token![,]>()?;
                        }
                    }
                }
            } else {
                return Err(Error::new(
                    keyword.span(),
                    format!(
                        "besu: unrecognised index attribute `{keyword}`; expected `columns`, \
                         `name`, `unique`, `using` or `opclass`"
                    ),
                ));
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        if this.entries.is_empty() {
            return Err(Error::new(
                Span::call_site(),
                "besu: `index` needs `columns(...)`",
            ));
        }

        if let Some(opclass) = &this.bare_opclass {
            if this.entries.len() != 1 {
                return Err(Error::new(
                    opclass.span(),
                    "besu: a bare `opclass` needs an index with exactly one entry; use \
                     `opclass(column = \"...\")` for a composite index",
                ));
            }
            this.entries[0].opclass = Some(opclass.clone());
        }

        for (column, opclass) in opclasses {
            let entry = this
                .entries
                .iter_mut()
                .find(|entry| {
                    matches!(&entry.target, IndexTargetDecl::Column(name) if name == &column.to_string())
                })
                .ok_or_else(|| {
                    Error::new(
                        opclass.span(),
                        format!("besu: no index column named `{column}`"),
                    )
                })?;
            if entry.opclass.is_some() {
                return Err(Error::new(
                    opclass.span(),
                    format!("besu: the operator class for `{column}` is given twice"),
                ));
            }
            entry.opclass = Some(opclass);
        }

        Ok(this)
    }
}

/// Classify one `columns(...)` entry as a column or an expression.
///
/// A bare identifier is a column. Everything else — a function call, arithmetic, a cast — is
/// an expression and is rendered back into SQL. A string literal is an escape hatch for an
/// expression the token printer would not preserve, and its value is used verbatim.
fn index_entry(expr: &Expr) -> Result<IndexEntryDecl> {
    match expr {
        Expr::Path(path)
            if path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1
                && path.path.segments[0].arguments.is_none() =>
        {
            Ok(IndexEntryDecl {
                target: IndexTargetDecl::Column(path.path.segments[0].ident.to_string()),
                opclass: None,
                span: path.span(),
            })
        }
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(value) => Ok(IndexEntryDecl {
                target: IndexTargetDecl::Expression(value.value()),
                opclass: None,
                span: value.span(),
            }),
            _ => Ok(IndexEntryDecl {
                target: IndexTargetDecl::Expression(render_expression(expr)),
                opclass: None,
                span: lit.span(),
            }),
        },
        other => Ok(IndexEntryDecl {
            target: IndexTargetDecl::Expression(render_expression(other)),
            opclass: None,
            span: other.span(),
        }),
    }
}

/// Render an attribute expression back into readable SQL.
///
/// `syn` does not keep the whitespace the caller wrote, so the token stream is printed and
/// then tidied: the spaces `to_string` puts around parentheses, commas and dots are removed,
/// while the spaces around an operator are kept so the expression still resembles SQL. A
/// string literal is data and is never touched.
fn render_expression(expr: &Expr) -> String {
    let raw = quote!(#expr).to_string();
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    out.push(chars.next().unwrap_or('\''));
                } else {
                    in_string = false;
                }
            }
            continue;
        }
        if ch == '\'' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch.is_whitespace() {
            let next = *chars.peek().unwrap_or(&' ');
            let previous = out.chars().last().unwrap_or(' ');
            let drop = matches!(next, '(' | ')' | ',' | '.' | ']')
                || matches!(previous, '(' | '[' | '.' | ' ');
            if !drop {
                out.push(' ');
            }
            while chars.peek().is_some_and(|next| next.is_whitespace()) {
                chars.next();
            }
            continue;
        }
        out.push(ch);
    }

    out.trim().to_owned()
}

/// Map `using = "..."` onto an `IndexKind` variant.
fn index_kind(lit: &LitStr) -> Result<TokenStream> {
    let kind = match lit.value().to_ascii_lowercase().as_str() {
        "btree" | "b-tree" => quote!(Btree),
        "gin" => quote!(Gin),
        "gist" => quote!(Gist),
        "brin" => quote!(Brin),
        "fulltext" | "full-text" => quote!(FullText),
        "spatial" => quote!(Spatial),
        other => {
            return Err(Error::new(
                lit.span(),
                format!(
                    "besu: `using = \"{other}\"` is not an index access method; expected \
                     btree, gin, gist, brin, fulltext or spatial"
                ),
            ));
        }
    };
    Ok(kind)
}

/// The deterministic name for a table-level index that does not pass `name`.
///
/// Columns contribute their own name and expressions contribute their 1-based position, so
/// `columns(a, lower(email))` on `users` is `users_a_expr2_idx`.
fn default_index_name(table: &str, index: &IndexDecl) -> String {
    let mut name = String::from(table);
    for (position, entry) in index.entries.iter().enumerate() {
        name.push('_');
        match &entry.target {
            IndexTargetDecl::Column(column) => name.push_str(column),
            IndexTargetDecl::Expression(_) => {
                name.push_str(&format!("expr{}", position + 1));
            }
        }
    }
    name.push_str("_idx");
    name
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
    sql_type: Option<LitStr>,
    renamed_from: Option<LitStr>,
    references: Option<LitStr>,
    on_delete: Option<String>,
    on_update: Option<String>,
    generated: Option<LitStr>,
    generated_stored: Option<bool>,
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
                if meta.path.is_ident("generated") {
                    this.generated = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("generated_stored") || meta.path.is_ident("stored") {
                    let lit: syn::LitBool = meta.value()?.parse()?;
                    this.generated_stored = Some(lit.value());
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
                if meta.path.is_ident("renamed_from") {
                    this.renamed_from = Some(meta.value()?.parse()?);
                    return Ok(());
                }
                if meta.path.is_ident("sql_type") {
                    this.sql_type = Some(meta.value()?.parse()?);
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
                     `default_now`, `sql_type`, `renamed_from`, `references`, `on_delete`, \
                     `on_update`, `generated`, `stored`",
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
    Column(Box<Column>),
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
    renamed_from: Option<LitStr>,
    reference: Option<Reference>,
    generated: Option<Generated>,
    /// Whether the seed generator has an implementation for this column's `SqlType`.
    ///
    /// An explicit `sql_type` can name a user-defined type, and there is no honest way to
    /// invent a value for it. Such a column is left unset by `Seedable` and has to be given a
    /// value with `.set(..)`; the trait's `REQUIRED` list is what turns "forgot to override"
    /// into a clear error instead of a constraint violation.
    seed_supported: bool,
}

/// A field computed by the database rather than supplied by the program.
struct Generated {
    expression: LitStr,
    stored: bool,
}

impl Column {
    /// Whether the database computes this column, so an insert or update must not.
    fn is_generated(&self) -> bool {
        self.generated.is_some()
    }
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

        // An explicit `sql_type` names a `SqlType` implementation directly, which is how a
        // `DECIMAL(10,2)`, a `VARCHAR(255)` or a user-defined type gets declared.
        let mut explicit_ty: Option<Type> = None;
        let explicit = match &attrs.sql_type {
            Some(lit) => {
                let ty: Type = syn::parse_str(&lit.value()).map_err(|err| {
                    Error::new(lit.span(), format!("besu: `sql_type` is not a type: {err}"))
                })?;
                explicit_ty = Some(ty.clone());
                Some((quote!(#ty), quote!(<#ty as #besu::SqlType>::DEF)))
            }
            None => None,
        };

        // Every type the derive infers from a Rust field has a generator. Only an explicit
        // `sql_type` can escape the built-in set, and then it is the caller's to fill in.
        let seed_supported = explicit_ty.as_ref().is_none_or(seed_type_supported);

        let (sql_ty, type_def) = match explicit {
            Some(explicit) => explicit,
            None => sql_type(inner, besu).map_err(|err| {
                Error::new(
                    err.span(),
                    format!(
                        "besu: `{}` is not a supported column type. Supported types are signed \
                     and unsigned integers (including u128 and i128), f32/f64, bool, String, \
                     Vec<u8>, uuid::Uuid, \
                     chrono::NaiveDate/NaiveTime/NaiveDateTime/DateTime<Utc> and \
                     serde_json::Value, each optionally wrapped in Option.",
                        quote!(#inner)
                    ),
                )
            })?,
        };

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

        if let Some(stored) = attrs.generated_stored
            && attrs.generated.is_none()
        {
            let _ = stored;
            return Err(Error::new(
                ident.span(),
                "besu: `stored` only applies to a `generated` column",
            ));
        }

        if let Some(expression) = &attrs.generated {
            if expression.value().trim().is_empty() {
                return Err(Error::new(
                    expression.span(),
                    "besu: a generated column needs an SQL expression",
                ));
            }
            // These are all rejected by at least one of the three dialects, and a schema
            // that only one of them accepts is not a schema Besu can promise to push.
            if attrs.primary_key || attrs.auto_increment {
                return Err(Error::new(
                    ident.span(),
                    "besu: a generated column cannot be a primary key or auto-increment",
                ));
            }
            if attrs.default.is_some() {
                return Err(Error::new(
                    ident.span(),
                    "besu: a generated column cannot have a default of its own",
                ));
            }
            if attrs.unique {
                return Err(Error::new(
                    ident.span(),
                    "besu: SQLite cannot put a UNIQUE constraint on a generated column",
                ));
            }
        }

        let reference = match &attrs.references {
            Some(value) => {
                let text = value.value();
                // Split at the last dot so `other.users.id` names a table in another schema
                // as well as a column.
                let Some((table, column)) = text.rsplit_once('.') else {
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

        Ok(Parsed::Column(Box::new(Column {
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
            renamed_from: attrs.renamed_from,
            reference,
            generated: attrs.generated.map(|expression| Generated {
                expression,
                stored: attrs.generated_stored.unwrap_or(true),
            }),
            seed_supported,
        })))
    }
}

/// Whether `besu::seed::SeedValue` has an implementation for this SQL type.
///
/// The list mirrors the implementations in `besu/src/seed.rs`. A type that is not here is not
/// a compile error: the column is left unset and the caller supplies it with `.set(..)`,
/// which keeps a table with one exotic column seedable.
fn seed_type_supported(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };

    match segment.ident.to_string().as_str() {
        "Bool" | "Int1" | "Int2" | "Int4" | "Int8" | "UInt1" | "UInt2" | "UInt4" | "UInt8"
        | "U128" | "I128" | "Float4" | "Float8" | "Text" | "Char" | "VarChar" | "Decimal"
        | "Bytes" | "Date" | "Time" | "Timestamp" | "TimestampTz" | "Uuid" | "Json" | "Jsonb" => {
            true
        }
        "Array" => match &segment.arguments {
            syn::PathArguments::AngleBracketed(args) => match args.args.first() {
                Some(syn::GenericArgument::Type(inner)) => seed_type_supported(inner),
                _ => false,
            },
            _ => false,
        },
        _ => false,
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
        "u128" => (quote!(#besu::U128), quote!(#besu::TypeDef::UInt(128))),
        "i128" => (quote!(#besu::I128), quote!(#besu::TypeDef::Decimal(39, 0))),
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
            let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
                return Err(Error::new(ty.span(), "unsupported"));
            };
            if let Type::Path(inner) = inner
                && inner
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "u8")
            {
                (quote!(#besu::Bytes), quote!(#besu::TypeDef::Bytes))
            } else {
                // A `Vec` of a scalar is an array. The element's `SqlType` is resolved by the
                // same mapping a bare field would use, so `Vec<i32>` is `Array<Int4>` and
                // `Vec<String>` is `Array<Text>`.
                let (element, element_def) = sql_type(inner, besu)?;
                (
                    quote!(#besu::Array<#element>),
                    quote!(#besu::TypeDef::Array(#besu::besu_core::types::TypeRef::from_static(
                        &#element_def
                    ))),
                )
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
    indexes: Vec<IndexDecl>,
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
        // A generated column is computed by the database, so it has no place in an insert
        // struct: there is nothing to set, and writing one is an error. Everything that
        // describes a row — the handle, the decode order and `model_values` — still sees it.
        let writable: Vec<&Column> = columns
            .iter()
            .filter(|column| !column.is_generated())
            .collect();

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
            // A generated column is produced by the database exactly as an identity column
            // is, and the query layer uses this flag to leave it out of a whole-row insert
            // or update. The distinction between the two spellings lives in `generated`.
            let auto_increment = column.auto_increment || column.is_generated();
            let primary_key = column.primary_key;
            let unique = column.unique;
            let renamed_from = match &column.renamed_from {
                Some(previous) => quote!(Some(#previous)),
                None => quote!(None),
            };
            let default = match &column.default {
                Some(Default::Literal(literal)) => {
                    quote!(Some(#besu::DefaultValue::#literal))
                }
                Some(Default::Raw(literal)) => quote!(Some(#besu::DefaultValue::Raw(#literal))),
                Some(Default::Now) => quote!(Some(#besu::DefaultValue::Now)),
                None => quote!(None),
            };
            let generated = match &column.generated {
                Some(generated) => {
                    let expression = &generated.expression;
                    let stored = generated.stored;
                    quote!(Some(#besu::Generated {
                        expression: #expression,
                        stored: #stored,
                    }))
                }
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
                    generated: #generated,
                    renamed_from: #renamed_from,
                }
            }
        });

        let field_index_defs = columns.iter().filter(|column| column.index).map(|column| {
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
                    entries: &[#besu::IndexEntry::column(#column_name)],
                    unique: false,
                    kind: #besu::IndexKind::Btree,
                }
            }
        });

        let declared_index_defs = self.indexes.iter().map(|index| {
            let name = match &index.name {
                Some(name) => name.clone(),
                None => LitStr::new(
                    &default_index_name(&self.table_name.value(), index),
                    index
                        .entries
                        .first()
                        .map_or_else(Span::call_site, |entry| entry.span),
                ),
            };
            let unique = index.unique;
            let kind = index
                .kind
                .as_ref()
                .map_or_else(|| quote!(Btree), |kind| quote!(#kind));
            let entries = index.entries.iter().map(|entry| {
                let base = match &entry.target {
                    IndexTargetDecl::Column(column) => {
                        let column = LitStr::new(column, entry.span);
                        quote!(#besu::IndexEntry::column(#column))
                    }
                    IndexTargetDecl::Expression(expression) => {
                        let expression = LitStr::new(expression, entry.span);
                        quote!(#besu::IndexEntry::expression(#expression))
                    }
                };
                match &entry.opclass {
                    Some(opclass) => quote!(#base.with_opclass(#opclass)),
                    None => base,
                }
            });
            quote! {
                #besu::Index {
                    name: #name,
                    entries: &[#(#entries),*],
                    unique: #unique,
                    kind: #besu::IndexKind::#kind,
                }
            }
        });

        let index_defs = field_index_defs.chain(declared_index_defs);

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
        // The insert struct has one field per writable column, so its value buffer is
        // shorter than the row's column count whenever a generated column exists.
        let writable_count = writable.len();

        // --- insert struct ------------------------------------------------------------
        let insert_fields = writable.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.ty;
            quote!(pub #ident: #besu::Set<#ty>)
        });

        let required: Vec<&&Column> = writable
            .iter()
            .filter(|column| !column.nullable && !column.auto_increment && column.default.is_none())
            .collect();

        // Required arguments accept anything convertible into the column's type, so
        // `users.insert("a@example.com", "Oscar", 21)` works without `.into()` noise.
        let required_args = required.iter().map(|column| {
            let ident = &column.ident;
            let ty = &column.ty;
            quote!(#ident: impl ::core::convert::Into<#ty>)
        });

        let insert_initialisers = writable.iter().map(|column| {
            let ident = &column.ident;
            if required
                .iter()
                .any(|required| required.ident == column.ident)
            {
                quote!(#ident: #besu::Set::Value(#ident.into()))
            } else {
                quote!(#ident: #besu::Set::Unset)
            }
        });

        let setters = writable.iter().map(|column| {
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

        let insert_columns = writable.iter().map(|column| &column.sql_name);
        let insert_values = writable.iter().map(|column| {
            let ident = &column.ident;
            let sql_ty = &column.sql_ty;
            let ty = &column.ty;
            // The column's SQL type is what decides how the Rust value is bound: `String`
            // is `TEXT` in one column and `NUMERIC` in another.
            quote!(#besu::expr::set_to_value::<#sql_ty, #ty>(&self.#ident))
        });
        // The same values, written straight into the statement's buffer. `self` is consumed
        // either way, so a derived row never materialises the intermediate `Vec` that
        // `into_values` returns.
        let insert_values_sink = writable.iter().map(|column| {
            let ident = &column.ident;
            let sql_ty = &column.sql_ty;
            let ty = &column.ty;
            quote!(out.push(#besu::expr::set_to_value::<#sql_ty, #ty>(&self.#ident));)
        });

        let from_model = writable.iter().map(|column| {
            let ident = &column.ident;
            if column.auto_increment {
                quote!(#ident: #besu::Set::Unset)
            } else if column.nullable {
                quote!(#ident: model.#ident.into())
            } else {
                quote!(#ident: #besu::Set::Value(model.#ident))
            }
        });

        // --- seeding ------------------------------------------------------------------
        // A column with no generator is left unset; when SQL would also refuse a missing
        // value, it is listed so the seeder can say which `.set(..)` is needed.
        let seed_required = writable
            .iter()
            .filter(|column| {
                !column.seed_supported
                    && !column.nullable
                    && !column.auto_increment
                    && column.default.is_none()
            })
            .map(|column| &column.sql_name);
        let seed_fields = writable.iter().map(|column| {
            let ident = &column.ident;
            let sql_ty = &column.sql_ty;
            let name = &column.sql_name;
            if !column.seed_supported {
                quote!(#ident: #besu::Set::Unset)
            } else if let Some(reference) = &column.reference {
                // The parent's key pool is keyed by the names in the reference, which is
                // exactly what the foreign key metadata already carries.
                let table = LitStr::new(&reference.table, column.ident.span());
                let referenced = LitStr::new(&reference.column, column.ident.span());
                quote!(#ident: #besu::Set::Value(
                    ctx.foreign_key::<#sql_ty>(#name, #table, #referenced)?
                ))
            } else if column.auto_increment || column.primary_key {
                // Integers number from 1; every other primary key is made unique.
                quote!(#ident: #besu::Set::Value(ctx.primary::<#sql_ty>(#name)))
            } else if column.unique {
                quote!(#ident: #besu::Set::Value(ctx.unique::<#sql_ty>(#name)))
            } else if column.nullable {
                quote!(#ident: ctx.optional::<#sql_ty>(#name))
            } else {
                quote!(#ident: #besu::Set::Value(ctx.value::<#sql_ty>(#name)))
            }
        });

        let model_doc = format!(
            "Insert a new row into the `{}` table.",
            self.table_name.value()
        );

        let model_values = columns.iter().map(|column| {
            let ident = &column.ident;
            let sql_ty = &column.sql_ty;
            let ty = &column.ty;
            if column.nullable {
                quote!(#besu::expr::option_to_value::<#sql_ty, #ty>(model.#ident.clone()))
            } else {
                quote!(#besu::expr::value_to_value::<#sql_ty, #ty>(model.#ident.clone()))
            }
        });

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

                impl #besu::Table for #table_struct_ident {
                    type Model = super::#model;
                    type Insert = #insert_struct_ident;

                    fn model_values(
                        model: &super::#model,
                    ) -> ::std::vec::Vec<#besu::Value> {
                        ::std::vec![#(#model_values),*]
                    }

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

                    fn write_values(
                        self,
                        out: &mut ::std::vec::Vec<::core::option::Option<#besu::Value>>,
                    ) {
                        out.reserve(#writable_count);
                        #(#insert_values_sink)*
                    }
                }

                impl ::core::convert::From<super::#model> for #insert_struct_ident {
                    fn from(model: super::#model) -> Self {
                        Self {
                            #(#from_model,)*
                        }
                    }
                }

                impl #besu::seed::Seedable for #table_struct_ident {
                    const REQUIRED: &'static [&'static str] = &[#(#seed_required),*];

                    fn seed_row(
                        ctx: &mut #besu::seed::SeedContext<'_>,
                    ) -> ::core::result::Result<Self::Insert, #besu::seed::SeedError> {
                        ::core::result::Result::Ok(#insert_struct_ident {
                            #(#seed_fields,)*
                        })
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
