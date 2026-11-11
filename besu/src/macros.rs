//! Raw SQL that is still safe.
//!
//! [`sql!`] builds a [`Fragment`]: SQL text plus bound parameters. The `?` placeholders are
//! rewritten to whatever the target dialect uses (`$1` for PostgreSQL, `?` elsewhere) when
//! the fragment is rendered, so one call site works against every backend.
//!
//! The number of `?` placeholders is checked against the number of arguments at compile
//! time, so an arity mistake is a compile error rather than a runtime surprise.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;

use besu_core::{Dialect, Error, Sql, Value};

/// A raw SQL statement with its parameters.
///
/// Construct one with [`sql!`](crate::sql) or [`Fragment::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    sql: Cow<'static, str>,
    args: Vec<Value>,
    placeholders: Placeholders,
}

/// Where a fragment's placeholders came from.
///
/// A fragment built by `sql!` spells its parameters `?` and is rewritten for whichever
/// dialect renders it. A fragment built from a finished [`Sql`] is the opposite: the
/// placeholders are already numbered and the text may carry a driver-specific suffix, so it
/// is only valid for the dialect that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Placeholders {
    /// `?` placeholders, rewritten at render time.
    Rewrite,
    /// Already rendered by the named dialect.
    Rendered(&'static str),
    /// Two fragments that could not be joined were joined anyway.
    Mixed,
}

impl Fragment {
    /// A fragment with no parameters.
    pub fn new(sql: impl Into<Cow<'static, str>>) -> Self {
        Self {
            sql: sql.into(),
            args: Vec::new(),
            placeholders: Placeholders::Rewrite,
        }
    }

    /// A fragment with parameters. The SQL must contain one `?` per argument.
    pub fn with_args(sql: impl Into<Cow<'static, str>>, args: Vec<Value>) -> Self {
        Self {
            sql: sql.into(),
            args,
            placeholders: Placeholders::Rewrite,
        }
    }

    /// Wrap an already-built statement in more SQL.
    ///
    /// This is how a typed query is embedded in a statement that has no builder: `EXPLAIN`,
    /// `INSERT ... SELECT`, `CREATE TABLE ... AS`, or a `WITH` clause written by hand. The
    /// outer text must not add parameters, because the inner statement's placeholders are
    /// already numbered; anything the builder bound stays bound, in order.
    ///
    /// ```ignore
    /// let sql = db.select(users).filter(users.age.gt(18)).to_sql()?;
    /// let plan: Vec<String> = db
    ///     .execute(Fragment::from(sql).wrap("EXPLAIN ", ""))
    ///     .all()
    ///     .await?;
    /// ```
    pub fn wrap(mut self, prefix: &str, suffix: &str) -> Self {
        if !prefix.is_empty() || !suffix.is_empty() {
            let mut text = String::with_capacity(prefix.len() + self.sql.len() + suffix.len());
            text.push_str(prefix);
            text.push_str(&self.sql);
            text.push_str(suffix);
            self.sql = Cow::Owned(text);
        }
        self
    }

    /// Append another fragment.
    ///
    /// Both must come from the same place. A fragment whose `?` are still to be rewritten
    /// cannot be glued to one that is already rendered, because the latter's placeholders are
    /// numbered and the former's are not; joining them records an error at render time rather
    /// than producing SQL that happens to parse.
    pub fn push(&mut self, other: Fragment) {
        self.placeholders = match (&self.placeholders, &other.placeholders) {
            (Placeholders::Rewrite, Placeholders::Rewrite) => Placeholders::Rewrite,
            (Placeholders::Rendered(a), Placeholders::Rendered(b)) if a == b => {
                Placeholders::Rendered(a)
            }
            _ => Placeholders::Mixed,
        };
        self.sql.to_mut().push_str(&other.sql);
        self.args.extend(other.args);
    }

    /// The raw SQL text, with the source dialect's `?` placeholders.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The bound parameters.
    pub fn args(&self) -> &[Value] {
        &self.args
    }

    /// Render for a specific dialect.
    #[track_caller]
    pub fn render<D: Dialect>(&self) -> Sql<D> {
        // A statement a builder already rendered has its placeholders numbered, so rewriting
        // them would corrupt the text. It is valid only for the dialect that produced it.
        match self.placeholders {
            Placeholders::Rendered(name) if name != D::NAME => {
                let mut out = Sql::<D>::raw(self.sql.clone());
                out.set_error(Error::unsupported(alloc::format!(
                    "this statement was rendered for `{name}` and cannot be re-rendered for \
                     `{}`; build it against a `{}` dialect instead",
                    D::NAME,
                    D::NAME
                )));
                return out;
            }
            Placeholders::Mixed => {
                let mut out = Sql::<D>::raw(self.sql.clone());
                out.set_error(Error::unsupported(
                    "this fragment joins raw SQL with an already-rendered statement, which \
                     mixes unnumbered and numbered placeholders; build the whole statement \
                     against one dialect instead",
                ));
                return out;
            }
            Placeholders::Rendered(_) => {
                let mut out = Sql::<D>::raw(self.sql.clone());
                for arg in &self.args {
                    out.push_arg(arg.clone());
                }
                return out;
            }
            Placeholders::Rewrite => {}
        }

        let mut sql = String::with_capacity(self.sql.len() + 8);
        let mut index = 0usize;
        let mut quote: Option<char> = None;
        let mut chars = self.sql.chars().peekable();

        while let Some(ch) = chars.next() {
            if let Some(q) = quote {
                sql.push(ch);
                if ch == q {
                    if chars.peek() == Some(&q) {
                        // A doubled quote is an escape, not the end of the literal.
                        sql.push(chars.next().unwrap_or(ch));
                    } else {
                        quote = None;
                    }
                }
                continue;
            }

            match ch {
                '\'' | '"' | '`' => {
                    quote = Some(ch);
                    sql.push(ch);
                }
                // `??` is an escaped question mark, for operators such as PostgreSQL's
                // `jsonb ? 'key'` that would otherwise look like a parameter.
                '?' if chars.peek() == Some(&'?') => {
                    chars.next();
                    sql.push('?');
                }
                '?' => {
                    D::write_placeholder(&mut sql, index);
                    index += 1;
                }
                _ => sql.push(ch),
            }
        }

        let mut out = Sql::<D>::raw(sql);
        for arg in &self.args {
            out.push_arg(arg.clone());
        }
        if index != self.args.len() {
            out.set_error(Error::invalid_query(alloc::format!(
                "raw SQL has {index} placeholder(s) but {} parameter(s) were bound",
                self.args.len()
            )));
        }
        out.seal()
    }
}

impl<D: Dialect> From<Sql<D>> for Fragment
where
    D::Value: Into<Value>,
{
    /// Keep a finished statement as a fragment, placeholders and all.
    ///
    /// Every shipped dialect uses [`Value`] as its parameter type, so this is lossless. The
    /// text is *not* re-rendered: it already carries the dialect's placeholders and the
    /// driver's statement suffix, and [`Fragment::render`] refuses to hand it to another
    /// dialect.
    fn from(sql: Sql<D>) -> Self {
        let (text, args) = sql.into_parts();
        Self {
            sql: Cow::Owned(text),
            args: args.into_iter().map(Into::into).collect(),
            placeholders: Placeholders::Rendered(D::NAME),
        }
    }
}

impl From<&'static str> for Fragment {
    fn from(sql: &'static str) -> Self {
        Self::new(sql)
    }
}

impl From<String> for Fragment {
    fn from(sql: String) -> Self {
        Self::new(sql)
    }
}

/// Count `?` placeholders outside of quotes.
///
/// This is a `const fn` so the `sql!` macro can compare it against the argument count while
/// compiling.
pub const fn count_placeholders(sql: &str) -> usize {
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut count = 0;
    let mut quote: u8 = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if quote != 0 {
            if byte == quote {
                if index + 1 < bytes.len() && bytes[index + 1] == quote {
                    index += 1;
                } else {
                    quote = 0;
                }
            }
        } else if byte == b'\'' || byte == b'"' || byte == b'`' {
            quote = byte;
        } else if byte == b'?' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'?' {
                // An escaped question mark, which binds nothing.
                index += 1;
            } else {
                count += 1;
            }
        }
        index += 1;
    }

    count
}

/// Build a [`Fragment`] from raw SQL and bound parameters.
///
/// The SQL must be a string literal so the placeholder count can be checked at compile time.
/// Every `?` outside a quoted string or a `--` comment is a parameter, and `??` is an escaped
/// literal question mark — needed for operators such as PostgreSQL's `jsonb ? 'key'`.
///
/// ```ignore
/// let sql = besu::sql!(
///     "SELECT * FROM users WHERE age > ? AND name LIKE ?",
///     18,
///     "%oscar%",
/// );
/// let users: Vec<User> = db.execute(sql).all().await?;
/// ```
///
/// Passing the wrong number of parameters is a compile error.
#[macro_export]
macro_rules! sql {
    ($sql:literal $(, $arg:expr )* $(,)?) => {{
        const _: () = {
            let placeholders = $crate::__private::count_placeholders($sql);
            // `stringify!` counts the arguments without evaluating them, so `sql!` works
            // with runtime values, not just constants.
            let provided = 0usize $(+ { let _ = stringify!($arg); 1usize })*;
            ::core::assert!(
                placeholders == provided,
                "besu::sql!: the SQL has a different number of `?` placeholders than arguments"
            );
        };
        let args: $crate::__private::Vec<$crate::Value> =
            $crate::__private::Vec::from([$($crate::__private::into_value($arg)),*]);
        $crate::Fragment::with_args($sql, args)
    }};
}

/// Items the macros need but users should not touch.
#[doc(hidden)]
pub mod __private {
    pub use alloc::vec::Vec;

    use besu_core::Value;

    /// Convert an arbitrary value into a bound SQL value.
    pub fn into_value(value: impl Into<Value>) -> Value {
        value.into()
    }

    pub use super::count_placeholders;
}

#[cfg(test)]
mod tests {
    use super::count_placeholders;

    #[test]
    fn counts_placeholders() {
        assert_eq!(count_placeholders("SELECT 1"), 0);
        assert_eq!(count_placeholders("SELECT * FROM t WHERE a = ?"), 1);
        assert_eq!(count_placeholders("a = ? AND b = ?"), 2);
    }

    #[test]
    fn doubled_question_marks_are_literal() {
        assert_eq!(count_placeholders("SELECT data ?? 'key'"), 0);
        assert_eq!(count_placeholders("SELECT data ?? 'key' AND id = ?"), 1);
    }

    #[test]
    fn ignores_placeholders_inside_strings() {
        assert_eq!(count_placeholders("SELECT '?'"), 0);
        assert_eq!(count_placeholders(r#"SELECT "?" , ?"#), 1);
        assert_eq!(count_placeholders("SELECT 'it''s ?' , ?"), 1);
        assert_eq!(count_placeholders("SELECT '?', ?, '?'"), 1);
    }
}
