use alloc::{borrow::Cow, fmt, string::String, sync::Arc, vec::Vec};

use crate::{Dialect, Error, Placeholder, Value};

/// A fully rendered SQL statement with its bound parameters.
///
/// `Sql` is generic over a [`Dialect`] only, never over a driver: this is the type that
/// makes Besu driverless. You can build and inspect the exact SQL that will run without a
/// database anywhere in sight.
///
/// Parameters are encoded exactly once, when they are bound. Nothing is formatted into the
/// SQL string, so SQL injection is impossible by construction — there is no API that takes
/// an interpolated value.
///
/// # Example
/// ```
/// use besu_core::{Dialect, Sql};
/// # #[derive(Debug, Clone, Copy, PartialEq, Eq)] struct Pg;
/// # impl Dialect for Pg {
/// #     type Value = besu_core::Value;
/// #     const NAME: &'static str = "postgres";
/// #     const PLACEHOLDER: besu_core::dialect::Placeholder = besu_core::dialect::Placeholder::Dollar;
/// #     const SUPPORTS_RETURNING: bool = true;
/// #     const SUPPORTS_ILIKE: bool = true;
/// #     const SUPPORTS_FULL_JOIN: bool = true;
/// #     const UPSERT: besu_core::UpsertStyle = besu_core::UpsertStyle::OnConflict;
/// #     fn bind(value: besu_core::Value) -> Result<Self::Value, besu_core::Error> { Ok(value) }
/// #     fn write_placeholder(out: &mut String, index: usize) { out.push('$'); out.push_str(&(index + 1).to_string()); }
/// #     fn type_name(ty: besu_core::TypeDef) -> std::borrow::Cow<'static, str> { "TEXT".into() }
/// # }
/// let mut sql = Sql::<Pg>::raw("SELECT * FROM users WHERE id = ");
/// sql.bind(42_i64);
/// assert_eq!(sql.sql(), "SELECT * FROM users WHERE id = $1");
/// ```
pub struct Sql<D: Dialect> {
    sql: Cow<'static, str>,
    args: Vec<D::Value>,
    error: Option<Error>,
    /// A prefix applied to every table name this statement mentions.
    ///
    /// Multi-tenant deployments that share one physical database often name tables
    /// `tenant_42_users`. The prefix lives here rather than in the query builder because
    /// every table name — in a `FROM`, a `JOIN`, or a qualified column reference — is written
    /// through this type, which makes it the one place that can apply it consistently.
    prefix: Option<Arc<str>>,
}

impl<D: Dialect> Default for Sql<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect> Clone for Sql<D> {
    fn clone(&self) -> Self {
        Self {
            sql: self.sql.clone(),
            args: self.args.clone(),
            error: self.error.clone(),
            prefix: self.prefix.clone(),
        }
    }
}

impl<D: Dialect> fmt::Debug for Sql<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inspect())
    }
}

impl<D: Dialect> PartialEq for Sql<D> {
    fn eq(&self, other: &Self) -> bool {
        self.sql == other.sql && self.args == other.args && self.error == other.error
    }
}

impl<D: Dialect> Eq for Sql<D> {}

impl<D: Dialect> Sql<D> {
    /// An empty statement.
    pub fn new() -> Self {
        Self {
            sql: Cow::Borrowed(""),
            args: Vec::new(),
            error: None,
            prefix: None,
        }
    }

    /// An empty statement with room for `bytes` of SQL.
    ///
    /// Rendering a statement appends to a single `String`, and every growth copies
    /// everything written so far. A builder that knows roughly how large the finished
    /// statement will be can start at that size and never copy at all; guessing slightly
    /// high wastes a few hundred bytes, which is the cheaper mistake.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            sql: Cow::Owned(String::with_capacity(bytes)),
            args: Vec::new(),
            error: None,
            prefix: None,
        }
    }

    /// Reserve room for `count` bound parameters.
    pub fn reserve_args(&mut self, count: usize) {
        self.args.reserve(count);
    }

    /// A statement starting with raw SQL. The string is used verbatim, so never pass it
    /// anything derived from user input.
    pub fn raw(sql: impl Into<Cow<'static, str>>) -> Self {
        Self {
            sql: sql.into(),
            args: Vec::new(),
            error: None,
            prefix: None,
        }
    }

    /// Apply `prefix` to every table name this statement will write.
    pub fn set_table_prefix(&mut self, prefix: Option<Arc<str>>) {
        self.prefix = prefix;
    }

    /// The prefix applied to table names, if any.
    pub fn table_prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// Write a table name, prefixed and schema-qualified.
    pub fn push_table(&mut self, schema: Option<&str>, table: &str) {
        if let Some(schema) = schema {
            self.push_ident(schema);
            self.push_raw(".");
        }
        match &self.prefix {
            // The prefixed path concatenates because a quoted identifier is one unit; the
            // unprefixed path, which is every statement in a database without prefixes,
            // writes straight into the buffer.
            Some(prefix) => self.push_raw(&D::quote(&alloc::format!("{prefix}{table}"))),
            None => self.push_ident(table),
        }
    }

    /// Append raw SQL verbatim.
    pub fn push_raw(&mut self, sql: &str) {
        self.sql.to_mut().push_str(sql);
    }

    /// Append raw SQL verbatim, taking ownership when possible.
    pub fn push_owned(&mut self, sql: &'static str) {
        if let Cow::Borrowed(existing) = &self.sql {
            if existing.is_empty() {
                self.sql = Cow::Borrowed(sql);
                return;
            }
        }
        self.sql.to_mut().push_str(sql);
    }

    /// Append a single space, unless the statement is empty or already ends in whitespace.
    pub fn space(&mut self) {
        let s = self.sql.to_mut();
        if !s.is_empty() && !s.ends_with(char::is_whitespace) {
            s.push(' ');
        }
    }

    /// Append a quoted identifier.
    pub fn push_ident(&mut self, ident: &str) {
        D::write_ident(self.sql.to_mut(), ident);
    }

    /// Append a quoted, schema-qualified identifier.
    pub fn push_qualified(&mut self, table: &str, column: &str) {
        match &self.prefix {
            Some(prefix) => {
                let prefixed = alloc::format!("{prefix}{table}");
                let out = self.sql.to_mut();
                D::write_ident(out, &prefixed);
                out.push('.');
                D::write_ident(out, column);
            }
            None => {
                let out = self.sql.to_mut();
                D::write_ident(out, table);
                out.push('.');
                D::write_ident(out, column);
            }
        }
    }

    /// Bind a value, appending the dialect's placeholder.
    pub fn bind(&mut self, value: impl Into<Value>) {
        self.bind_value(value.into());
    }

    /// Bind an already-built [`Value`].
    pub fn bind_value(&mut self, value: Value) {
        let index = self.args.len();
        D::write_placeholder(self.sql.to_mut(), index);
        match D::bind(value) {
            Ok(value) => {
                self.args.push(value);
                self.check_param_limit();
            }
            Err(err) => self.set_error(err),
        }
    }

    /// Record an error once the statement carries more parameters than the database accepts.
    ///
    /// Every database has a limit, and exceeding it produces a driver error that says nothing
    /// about which query or why. Catching it here costs one comparison per parameter.
    fn check_param_limit(&mut self) {
        if self.args.len() > D::MAX_PARAMS {
            self.set_error(Error::invalid_query(alloc::format!(
                "this statement binds more than {} parameters, which is the most `{}` accepts; \
                 split it into batches",
                D::MAX_PARAMS,
                D::NAME
            )));
        }
    }

    /// Push a parameter without writing a placeholder.
    ///
    /// This is for raw SQL where the placeholders are already in the text, as produced by
    /// the `sql!` macro.
    pub fn push_arg(&mut self, value: Value) {
        match D::bind(value) {
            Ok(value) => {
                self.args.push(value);
                self.check_param_limit();
            }
            Err(err) => self.set_error(err),
        }
    }

    /// Append another statement, moving across its parameters and any error.
    ///
    /// The appended statement keeps working for a dialect that numbers its placeholders: each
    /// index in the appended text is moved along by the number of parameters this statement
    /// already carries. Placeholders inside a single-quoted, double-quoted or backtick-quoted
    /// literal are left alone.
    pub fn append(&mut self, mut other: Sql<D>) {
        let offset = self.args.len();
        {
            let tail = other.sql.to_mut();
            match D::PLACEHOLDER {
                // Every placeholder is a bare `?`, and the next one is whatever comes next.
                Placeholder::Question => {}
                Placeholder::Dollar => renumber_placeholders(tail, '$', offset),
                Placeholder::QuestionNumbered => renumber_placeholders(tail, '?', offset),
            }
            let out = self.sql.to_mut();
            out.push_str(tail);
        }
        self.args.append(&mut other.args);
        if self.error.is_none() {
            self.error = other.error;
        }
        // The two statements were each within the limit on their own, but together they may
        // not be.
        self.check_param_limit();
    }

    /// The SQL text.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// The bound parameters.
    pub fn args(&self) -> &[D::Value] {
        &self.args
    }

    /// Take the bound parameters.
    pub fn into_args(self) -> Vec<D::Value> {
        self.args
    }

    /// Take the SQL text and parameters.
    pub fn into_parts(self) -> (String, Vec<D::Value>) {
        (self.sql.into_owned(), self.args)
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.sql.is_empty()
    }

    /// The first error recorded while building, if any.
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    /// Record an error. Only the first one is kept, because later failures are almost
    /// always a consequence of the first.
    pub fn set_error(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// Fail with `error` if any error has been recorded.
    pub fn check(&self) -> Result<(), Error> {
        match &self.error {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    /// Take the finished statement, failing if building it recorded an error.
    pub fn finish(self) -> Result<Self, Error> {
        match self.error {
            Some(err) => Err(err),
            None => Ok(self),
        }
    }

    /// Append the dialect's statement suffix, if it has one.
    ///
    /// Called once, at the end of building a statement, so the suffix is part of the text that
    /// `to_sql` shows and part of the string the driver sends. See
    /// [`Dialect::write_statement_suffix`] for why it exists.
    pub fn seal(mut self) -> Self {
        D::write_statement_suffix(self.sql.to_mut(), &self.args);
        self
    }

    /// Render the statement for a driver.
    pub fn render(&self) -> Result<Rendered<'_, D>, Error> {
        self.check()?;
        Ok(Rendered {
            sql: &self.sql,
            args: &self.args,
        })
    }

    /// A human-readable rendering with parameters substituted, for logging and `EXPLAIN`.
    ///
    /// This is best-effort: it understands quoted strings well enough to avoid the common
    /// traps, but it is a debugging aid, never something to execute.
    pub fn inspect(&self) -> Inspect<'_, D> {
        Inspect(self)
    }
}

/// A borrowed, ready-to-execute statement.
#[derive(Debug, Clone, Copy)]
pub struct Rendered<'a, D: Dialect> {
    /// The SQL text.
    pub sql: &'a str,
    /// The bound parameters.
    pub args: &'a [D::Value],
}

/// A [`Sql`] rendered with its parameters inlined. See [`Sql::inspect`].
pub struct Inspect<'a, D: Dialect>(&'a Sql<D>);

impl<D: Dialect> fmt::Display for Inspect<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sql = &self.0.sql;
        let mut params = self.0.args.iter();
        let mut quote: Option<char> = None;
        let mut chars = sql.char_indices().peekable();

        while let Some((_, ch)) = chars.next() {
            if let Some(q) = quote {
                f.write_fmt(format_args!("{ch}"))?;
                if ch == q {
                    quote = None;
                }
                continue;
            }

            match ch {
                '\'' | '"' | '`' => {
                    quote = Some(ch);
                    f.write_fmt(format_args!("{ch}"))?;
                }
                '?' => {
                    // A dialect with numbered placeholders never writes `?` for a parameter,
                    // so a bare `?` here is SQL — PostgreSQL's `jsonb ? 'key'` operator. It
                    // is only ambiguous for a dialect that spells parameters `?`, and only
                    // there is it substituted.
                    if D::PLACEHOLDER == crate::Placeholder::Dollar {
                        f.write_str("?")?;
                        continue;
                    }
                    // Consume an optional explicit index (`?1`), which we ignore because
                    // parameters arrive in order anyway.
                    let digits = take_digits(&mut chars);
                    match params.next() {
                        Some(value) => f.write_fmt(format_args!("{}", render_param::<D>(value)))?,
                        // Nothing to substitute: write the placeholder back exactly as it was.
                        None => {
                            f.write_str("?")?;
                            f.write_str(&digits)?;
                        }
                    }
                }
                '$' => {
                    let digits = take_digits(&mut chars);
                    if digits.is_empty() {
                        f.write_str("$")?;
                    } else {
                        match params.next() {
                            Some(value) => {
                                f.write_fmt(format_args!("{}", render_param::<D>(value)))?
                            }
                            None => {
                                f.write_str("$")?;
                                f.write_str(&digits)?;
                            }
                        }
                    }
                }
                _ => f.write_fmt(format_args!("{ch}"))?,
            }
        }

        if self.0.args.is_empty() && sql.is_empty() {
            return Ok(());
        }

        for value in params {
            f.write_fmt(format_args!(" /* {} */", render_param::<D>(value)))?;
        }

        Ok(())
    }
}

/// Move every numbered placeholder in `sql` along by `offset`.
///
/// `marker` is the character a numbered placeholder starts with: `$` for PostgreSQL and `?`
/// for SQLite's explicit form. A marker that is not followed by a digit is SQL text — a `?`
/// operator, a `$` in an identifier — and is left alone, as is everything inside a quoted
/// literal.
fn renumber_placeholders(sql: &mut String, marker: char, offset: usize) {
    if offset == 0 || !sql.contains(marker) {
        return;
    }
    let mut out = String::with_capacity(sql.len());
    let mut quote: Option<char> = None;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(open) = quote {
            out.push(ch);
            if ch == open {
                quote = None;
            }
            continue;
        }
        if marker != ch {
            if matches!(ch, '\'' | '"' | '`') {
                quote = Some(ch);
            }
            out.push(ch);
            continue;
        }
        // The marker starts a placeholder only when a digit follows it.
        if !chars.peek().is_some_and(char::is_ascii_digit) {
            out.push(ch);
            continue;
        }
        let mut digits = String::new();
        while let Some(&next) = chars.peek() {
            if !next.is_ascii_digit() {
                break;
            }
            digits.push(next);
            chars.next();
        }
        out.push(marker);
        match digits.parse::<usize>() {
            Ok(index) => crate::dialect::write_index(&mut out, index.saturating_add(offset)),
            // Too many digits to be an index Besu wrote; keep the text as it stands.
            Err(_) => out.push_str(&digits),
        }
    }
    *sql = out;
}

/// Consume the run of ASCII digits that follows a placeholder marker.
///
/// The digits are returned rather than discarded so that a placeholder with nothing to
/// substitute is written back exactly as it appeared.
fn take_digits(chars: &mut core::iter::Peekable<core::str::CharIndices<'_>>) -> String {
    let mut digits = String::new();
    while let Some(&(_, ch)) = chars.peek() {
        if !ch.is_ascii_digit() {
            break;
        }
        digits.push(ch);
        chars.next();
    }
    digits
}

fn render_param<D: Dialect>(value: &D::Value) -> String {
    alloc::format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Placeholder, TypeDef, UpsertStyle};

    /// A PostgreSQL-shaped dialect: numbered placeholders that are part of the text.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Postgres;

    impl Dialect for Postgres {
        type Value = Value;
        const NAME: &'static str = "postgres";
        const PLACEHOLDER: Placeholder = Placeholder::Dollar;
        const SUPPORTS_RETURNING: bool = true;
        const SUPPORTS_ILIKE: bool = true;
        const SUPPORTS_FULL_JOIN: bool = true;
        const UPSERT: UpsertStyle = UpsertStyle::OnConflict;

        fn bind(value: Value) -> Result<Self::Value, Error> {
            Ok(value)
        }

        fn write_placeholder(out: &mut String, index: usize) {
            out.push('$');
            crate::dialect::write_index(out, index + 1);
        }

        fn type_name(_: TypeDef) -> Cow<'static, str> {
            Cow::Borrowed("TEXT")
        }
    }

    /// A SQLite-shaped dialect with one `?` per parameter, plus a parameter limit.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Sqlite;

    impl Dialect for Sqlite {
        type Value = Value;
        const NAME: &'static str = "sqlite";
        const PLACEHOLDER: Placeholder = Placeholder::Question;
        const SUPPORTS_RETURNING: bool = true;
        const SUPPORTS_ILIKE: bool = false;
        const SUPPORTS_FULL_JOIN: bool = true;
        const UPSERT: UpsertStyle = UpsertStyle::OnConflict;
        const MAX_PARAMS: usize = 2;

        fn bind(value: Value) -> Result<Self::Value, Error> {
            Ok(value)
        }

        fn write_placeholder(out: &mut String, _: usize) {
            out.push('?');
        }

        fn type_name(_: TypeDef) -> Cow<'static, str> {
            Cow::Borrowed("TEXT")
        }
    }

    #[test]
    fn a_space_is_not_added_after_any_whitespace() {
        for (sql, expected) in [
            ("a", "a "),
            ("a ", "a "),
            ("a\n", "a\n"),
            ("a\t", "a\t"),
            ("", ""),
        ] {
            let mut sql = Sql::<Postgres>::raw(sql);
            sql.space();
            assert_eq!(sql.sql(), expected);
        }
    }

    #[test]
    fn appending_renumbers_numbered_placeholders() {
        // Each statement is built as its own text, so each starts at `$1`. Appending the two
        // has to move the second one along or the statement refers to the same parameter
        // twice and never mentions the second.
        let mut first = Sql::<Postgres>::raw("a = ");
        first.bind(1_i64);
        let mut second = Sql::<Postgres>::raw(" AND b = ");
        second.bind(2_i64);
        first.append(second);
        assert_eq!(first.sql(), "a = $1 AND b = $2");
        assert_eq!(first.args(), &[Value::Int(1), Value::Int(2)]);

        // A `$1` that is SQL text rather than a placeholder is left alone.
        let mut first = Sql::<Postgres>::raw("a = ");
        first.bind(1_i64);
        let mut second = Sql::<Postgres>::raw(" AND note = 'cost $1' AND b = ");
        second.bind(2_i64);
        first.append(second);
        assert_eq!(first.sql(), "a = $1 AND note = 'cost $1' AND b = $2");
    }

    #[test]
    fn appending_a_question_dialect_needs_no_renumbering() {
        let mut first = Sql::<Sqlite>::raw("a = ");
        first.bind(1_i64);
        let mut second = Sql::<Sqlite>::raw(" AND b = ");
        second.bind(2_i64);
        first.append(second);
        assert_eq!(first.sql(), "a = ? AND b = ?");
    }

    #[test]
    fn appending_cannot_sneak_past_the_parameter_limit() {
        // Each statement is within SQLite's own limit of two, but the pair is not.
        let mut first = Sql::<Sqlite>::raw("a = ");
        first.bind(1_i64);
        first.bind(2_i64);
        let mut second = Sql::<Sqlite>::raw(" AND b = ");
        second.bind(3_i64);
        second.bind(4_i64);
        first.append(second);
        assert!(first.error().is_some(), "the limit must be checked again");
        assert!(first.render().is_err());
    }

    #[test]
    fn inspecting_keeps_a_placeholder_that_has_no_parameter() {
        // A malformed statement is a debugging aid's problem, but the rendering must not
        // invent different SQL: the unmatched placeholder is written back as it was.
        let mut sql = Sql::<Postgres>::raw("SELECT $1, $2");
        sql.push_arg(Value::Int(1));
        assert_eq!(sql.inspect().to_string(), "SELECT 1, $2");

        let mut sql = Sql::<Sqlite>::raw("SELECT ?, ?");
        sql.push_arg(Value::Int(1));
        assert_eq!(sql.inspect().to_string(), "SELECT 1, ?");
    }

    #[test]
    fn inspecting_substitutes_each_parameter_once_in_order() {
        let mut sql = Sql::<Postgres>::raw("SELECT * FROM t WHERE a = ");
        sql.bind(1_i64);
        sql.push_raw(" AND b = ");
        sql.bind("x");
        assert_eq!(
            sql.inspect().to_string(),
            "SELECT * FROM t WHERE a = 1 AND b = 'x'"
        );
    }
}
