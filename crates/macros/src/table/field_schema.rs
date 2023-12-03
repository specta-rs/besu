use proc_macro2::TokenStream;
use syn::Expr;

use quote::{quote, ToTokens};

// TODO: Dealing with per-db type overrides
// TODO: Deal with mapping types to rust types
// TODO: Doing compile time parsing makes custom abstractions impossible which is cringe. Eg. Something like my Drizzle `const text = (name) => varchar(name, { length: 255 })`

#[derive(PartialEq, Eq)]
pub enum FieldType {
    Text,
    Serial,
    Integer,
    Uuid,
    DateTime,
    Unknown,
}

pub struct FieldSchema {
    pub ty: FieldType,
    pub optional: bool,
    pub default: Option<Expr>,
}

impl FieldSchema {
    pub fn new(expr: &Expr) -> Self {
        let mut this = Self {
            ty: FieldType::Unknown,
            optional: false,
            default: None,
        };

        Self::do_the_thing(&mut this, expr);

        match this.ty {
            FieldType::Unknown => panic!(
                "type was not set: '{:?}'",
                expr.into_token_stream().to_string(),
            ),
            _ => {}
        }

        this
    }

    fn do_the_thing(this: &mut Self, expr: &Expr) {
        match expr {
            Expr::Array(_) => todo!("a"),
            Expr::Assign(_) => todo!("b"),
            Expr::Async(_) => todo!("c"),
            Expr::Await(_) => todo!("d"),
            Expr::Binary(_) => todo!("e"),
            Expr::Block(_) => todo!("f"), // TODO: Support this
            Expr::Break(_) => todo!("g"),
            Expr::Call(_) => todo!("h"),
            Expr::Cast(_) => todo!("i"),
            Expr::Closure(_) => todo!("j"),
            Expr::Const(_) => todo!("k"),
            Expr::Continue(_) => todo!("l"),
            Expr::Field(_) => todo!("m"),
            Expr::ForLoop(_) => todo!("n"),
            Expr::Group(_) => todo!("o"),
            Expr::If(_) => todo!("p"),
            Expr::Index(_) => todo!("q"),
            Expr::Infer(_) => todo!("r"),
            Expr::Let(_) => todo!("s"),
            Expr::Lit(_) => todo!("t"),
            Expr::Loop(_) => todo!("u"),
            Expr::Macro(_) => todo!("v"),
            Expr::Match(_) => todo!("x"),
            Expr::MethodCall(call) => {
                Self::do_the_thing(this, &call.receiver);

                // TODO: How do we handle per-db type mappings
                match call.method.to_string().as_str() {
                    "text" => {
                        // TODO: Parse arguments for column name.

                        this.ty = FieldType::Text;
                    }
                    "serial" => {
                        // TODO: Parse arguments for column name.

                        this.ty = FieldType::Serial;
                    }
                    "int" => {
                        // TODO: Parse arguments for column name.

                        this.ty = FieldType::Integer;
                    }
                    "uuid" => {
                        // TODO: Parse arguments for column name.

                        this.ty = FieldType::Uuid;
                    }
                    "datetime" => {
                        // TODO: Parse arguments for column name.

                        this.ty = FieldType::DateTime;
                    }
                    "optional" => {
                        this.optional = true;
                    }
                    "default" => {
                        let expr = call
                            .args
                            .iter()
                            .next()
                            .expect("default() requires an argument")
                            .clone();

                        this.default = Some(expr);
                    }
                    _ => todo!("unrecognized method '{:?}'", call.method),
                }
            }
            Expr::Paren(_) => todo!("1"),
            Expr::Path(_) => {} // `|t| t` would match this. Avoiding the panic might help LSP hints.
            Expr::Range(_) => todo!("3"),
            Expr::Reference(_) => todo!("4"),
            Expr::Repeat(_) => todo!("5"),
            Expr::Return(_) => todo!("6"),
            Expr::Struct(_) => todo!("7"),
            Expr::Try(_) => todo!("8"),
            Expr::TryBlock(_) => todo!("9"),
            Expr::Tuple(_) => todo!("10"),
            Expr::Unary(_) => todo!("11"),
            Expr::Unsafe(_) => todo!("12"),
            Expr::Verbatim(_) => todo!("13"),
            Expr::While(_) => todo!("14"),
            Expr::Yield(_) => todo!("15"),
            _ => unreachable!(),
        }
    }

    pub fn rust_ty(&self) -> TokenStream {
        let mut ty = match self.ty {
            FieldType::Text => quote! { String },
            FieldType::Serial => quote! { u64 }, // TODO: Which is the best Rust integer type for this???
            FieldType::Integer => quote! { u64 },
            FieldType::Uuid => quote! { uuid::Uuid },
            FieldType::DateTime => quote! { String }, // TODO: chrono::DateTime<chrono::Utc>
            FieldType::Unknown => unreachable!(),
        };

        if self.optional {
            ty = quote! { Option<#ty> }
        }

        ty
    }
}
