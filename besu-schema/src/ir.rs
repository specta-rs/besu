use besu_core::{ColumnDef, DefaultValue, TableDef, TypeDef};
use serde::{Deserialize, Serialize};

/// A complete database schema.
///
/// Tables are kept in a `Vec` rather than a map so that iteration order — and therefore
/// generated DDL — is deterministic. Lookup is linear, which is irrelevant for schemas of
/// even a thousand tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// Every table in the schema.
    #[serde(default)]
    pub tables: Vec<Table>,
    /// Schema-level named enums, kept for round-tripping databases that have them.
    ///
    /// Only PostgreSQL has types that are not part of a column. The differ creates these
    /// before the tables that use them and drops them after, and a column of an enum type
    /// is `TypeDef::Custom`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enums: Vec<Enum>,
    /// Views and materialised views.
    ///
    /// A view carries its definition as SQL, because a query is not something the IR can
    /// reconstruct. The differ compares the definition text (after folding away the
    /// differences a database introduces when it rewrites one), creates a missing view
    /// after the tables it reads, replaces a changed one and drops a removed one before the
    /// tables it depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<View>,
    /// Standalone sequences, which only PostgreSQL has.
    ///
    /// The sequence behind a `SERIAL` or `IDENTITY` column is not listed here: it belongs to
    /// its column and is modelled by [`Column::auto_increment`]. This list is for the ones a
    /// person creates and names, which may be shared between tables or pre-allocated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<Sequence>,
    /// Row-level security policies, kept for round-tripping only.
    ///
    /// A policy is raw SQL attached to a table: its `USING` and `WITH CHECK` clauses cannot
    /// be compared safely, so the differ **ignores** this list entirely. A policy is
    /// therefore never created or dropped by `push`; introspecting it means a `besu pull`
    /// snapshot records what is really there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<Policy>,
}

/// A view, as declared by a snapshot or reported by a database.
///
/// The definition is SQL: a `SELECT` body for PostgreSQL and MySQL, and the whole
/// `CREATE VIEW` statement for SQLite. Either spelling is accepted when rendering DDL, and
/// [`normalise_view_definition`] folds them together for comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The view's name, qualified by its schema the same way a [`Table`] is.
    pub name: String,
    /// The dialect's own description of the view.
    pub definition: String,
    /// Whether this is a materialised view.
    ///
    /// PostgreSQL is the only dialect with them. MySQL and SQLite create an ordinary view
    /// instead, which is documented drift: introspection reports it as non-materialised.
    #[serde(default, skip_serializing_if = "is_false")]
    pub materialised: bool,
}

/// Serde helper: leave a `false` flag out of the JSON.
fn is_false(value: &bool) -> bool {
    !*value
}

impl View {
    /// A plain view with the given name and `SELECT` body.
    pub fn new(name: impl Into<String>, definition: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definition: definition.into(),
            materialised: false,
        }
    }

    /// The schema this view lives in, when its name is qualified.
    ///
    /// A view's identity is its name, exactly as it is for a table; a name written
    /// `app.active_users` lives in `app` and the one written `active_users` lives in the
    /// connection's default schema.
    pub fn schema(&self) -> Option<&str> {
        self.name.split_once('.').map(|(schema, _)| schema)
    }

    /// The `SELECT` body, with a leading `CREATE VIEW ... AS` removed.
    ///
    /// SQLite reports the entire statement, while PostgreSQL and MySQL report only the
    /// body. Rendering a view always needs the body, so the wrapper is stripped wherever it
    /// appears.
    pub fn body(&self) -> &str {
        view_body(&self.definition)
    }
}

/// The `SELECT` part of a view definition, with any `CREATE VIEW ... AS` wrapper removed.
///
/// This is public because the dialects render it and the differ compares it, and both have
/// to agree about what "the definition" means.
pub fn view_body(definition: &str) -> &str {
    let trimmed = definition.trim().trim_end_matches(';').trim_end();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("create ") {
        return trimmed;
    }
    // Find the ` AS ` that separates the header from the query. A definition written by
    // SQLite always has one; nothing before it can contain the word in an identifier
    // position, and the query itself comes afterwards.
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    while index + 4 <= bytes.len() {
        if bytes[index..index + 4].eq_ignore_ascii_case(b" as ") {
            return trimmed[index + 4..].trim();
        }
        index += 1;
    }
    trimmed
}

/// Fold the differences a database introduces when it rewrites a view definition.
///
/// Comparing the text verbatim would never converge: PostgreSQL qualifies every column with
/// its table, MySQL backticks every identifier and adds a redundant `AS` alias for each one,
/// and SQLite repeats the `CREATE VIEW` header. The comparison therefore drops identifier
/// quoting, table qualifiers, the `AS` aliases that only repeat their column, whitespace,
/// case outside string literals and a trailing semicolon. It is a best effort: a definition
/// the database rewrites in a way this cannot undo — `SELECT *` expanded to a column list,
/// for example — is reported as changed and re-created on each push.
pub fn normalise_view_definition(definition: &str) -> String {
    enum Token {
        Word(String),
        Literal(String),
        Punct(char),
    }

    fn tokenise(body: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut word = String::new();
        let mut chars = body.chars().peekable();

        fn flush(word: &mut String, tokens: &mut Vec<Token>) {
            if !word.is_empty() {
                tokens.push(Token::Word(core::mem::take(word)));
            }
        }

        while let Some(ch) = chars.next() {
            match ch {
                // A string literal is data: keep it exactly as written.
                '\'' => {
                    flush(&mut word, &mut tokens);
                    let mut literal = String::from("'");
                    while let Some(inner) = chars.next() {
                        literal.push(inner);
                        if inner == '\'' {
                            if chars.peek() == Some(&'\'') {
                                literal.push(chars.next().unwrap_or('\''));
                            } else {
                                break;
                            }
                        }
                    }
                    tokens.push(Token::Literal(literal));
                }
                // A quoted identifier is just a name; the quote carries no meaning here.
                '"' | '`' => {
                    flush(&mut word, &mut tokens);
                    let mut name = String::new();
                    for inner in chars.by_ref() {
                        if inner == ch {
                            break;
                        }
                        name.push(inner);
                    }
                    tokens.push(Token::Word(name));
                }
                ch if ch.is_alphanumeric() || ch == '_' || ch == '$' => word.push(ch),
                ch => {
                    flush(&mut word, &mut tokens);
                    if !ch.is_whitespace() {
                        tokens.push(Token::Punct(ch));
                    }
                }
            }
        }
        flush(&mut word, &mut tokens);

        // A quoted name can be followed by a dot outside the quotes (`` `users`.`id` ``),
        // which the scanner sees as three tokens. Joining them back makes qualifier folding
        // work the same way for every dialect; a name can have more than two segments, so
        // the pass repeats until nothing is left to join.
        let mut merged = tokens;
        loop {
            let before = merged.len();
            let mut joined = Vec::with_capacity(before);
            let mut remaining = merged.into_iter().peekable();
            while let Some(token) = remaining.next() {
                let Token::Word(word) = &token else {
                    joined.push(token);
                    continue;
                };
                if !matches!(remaining.peek(), Some(Token::Punct('.'))) {
                    joined.push(token);
                    continue;
                }
                remaining.next();
                match remaining.next() {
                    Some(Token::Word(next)) => joined.push(Token::Word(format!("{word}.{next}"))),
                    Some(other) => {
                        joined.push(token);
                        joined.push(Token::Punct('.'));
                        joined.push(other);
                    }
                    None => {
                        joined.push(token);
                        joined.push(Token::Punct('.'));
                    }
                }
            }
            merged = joined;
            if merged.len() == before {
                return merged;
            }
        }
    }

    /// The bare name of a token that can be an identifier.
    fn name(token: &Token) -> Option<&str> {
        match token {
            Token::Word(word) => Some(word.rsplit('.').next().unwrap_or(word)),
            _ => None,
        }
    }

    let tokens = tokenise(view_body(definition));

    // A qualifier says which table a name belongs to, which only matters once there is more
    // than one. In a single-table view it is redundant, so PostgreSQL's `users.id` folds to
    // `id`. Keeping it in a join is what stops `a.id` and `b.id` from looking equal.
    let joined = tokens
        .iter()
        .any(|token| name(token).is_some_and(|word| word.eq_ignore_ascii_case("join")));
    fn bare(word: &str, joined: bool) -> &str {
        if joined {
            word
        } else {
            word.rsplit('.').next().unwrap_or(word)
        }
    }

    // MySQL writes `users.id AS id`; the alias is the column's own name, so it says
    // nothing and must not look like a change.
    let mut dropped = vec![false; tokens.len()];
    for index in 0..tokens.len() {
        let Some(word) = name(&tokens[index]) else {
            continue;
        };
        if !word.eq_ignore_ascii_case("as") {
            continue;
        }
        let Some(alias) = tokens.get(index + 1).and_then(name) else {
            continue;
        };
        let previous = tokens[..index].iter().rev().find_map(|token| match token {
            Token::Word(word) => Some(bare(word, joined)),
            _ => None,
        });
        if previous.is_some_and(|previous| previous.eq_ignore_ascii_case(alias)) {
            dropped[index] = true;
            dropped[index + 1] = true;
        }
    }

    // PostgreSQL wraps a comparison in parentheses wherever it appears, so
    // `WHERE id > 0` comes back as `WHERE (id > 0)`. A pair is folded when it holds a
    // comparison and no boolean operator, which is exactly the redundant case; a pair that
    // groups `AND`/`OR` is left alone because removing it would change the meaning.
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..tokens.len() {
        match &tokens[index] {
            Token::Punct('(') => stack.push(index),
            Token::Punct(')') => {
                let Some(open) = stack.pop() else {
                    continue;
                };
                let mut depth = 0usize;
                let mut comparison = false;
                let mut boolean = false;
                for token in &tokens[open + 1..index] {
                    match token {
                        Token::Punct('(') => depth += 1,
                        Token::Punct(')') => depth = depth.saturating_sub(1),
                        Token::Punct('=' | '<' | '>') if depth == 0 => comparison = true,
                        Token::Word(word) if depth == 0 => {
                            match word
                                .rsplit('.')
                                .next()
                                .unwrap_or(word)
                                .to_ascii_lowercase()
                                .as_str()
                            {
                                "and" | "or" | "not" => boolean = true,
                                "like" | "ilike" | "is" | "in" | "between" => comparison = true,
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                if comparison && !boolean {
                    dropped[open] = true;
                    dropped[index] = true;
                }
            }
            _ => {}
        }
    }

    let mut out = String::with_capacity(definition.len());
    for (index, token) in tokens.iter().enumerate() {
        if dropped[index] {
            continue;
        }
        match token {
            Token::Literal(literal) => out.push_str(literal),
            Token::Punct(ch) => out.push(*ch),
            Token::Word(word) => {
                // A decimal literal's dot is not a qualifier.
                if word
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_digit())
                {
                    out.push_str(word);
                } else {
                    out.push_str(&bare(word, joined).to_lowercase());
                }
            }
        }
    }

    out
}

/// The view names a definition reads from, as far as a token scan can tell.
fn view_dependencies(view: &View) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = view.body().chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // A string literal is data, not a table name.
            '\'' => {
                if !current.is_empty() {
                    words.push(core::mem::take(&mut current));
                }
                while let Some(inner) = chars.next() {
                    if inner == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            // A quoted identifier is a name; keep its content.
            '"' | '`' => {
                for inner in chars.by_ref() {
                    if inner == ch {
                        break;
                    }
                    current.push(inner);
                }
                current.push('.');
            }
            ch if ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '.' => current.push(ch),
            _ => {
                if !current.is_empty() {
                    words.push(core::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    let mut out = Vec::new();
    for pair in words.windows(2) {
        let keyword = pair[0].to_ascii_lowercase();
        if keyword != "from" && keyword != "join" {
            continue;
        }
        let candidate = pair[1].trim_matches('.');
        if candidate.is_empty() {
            continue;
        }
        out.push(candidate.to_owned());
        if let Some((_, last)) = candidate.rsplit_once('.') {
            out.push(last.to_owned());
        }
    }
    out
}

/// A standalone sequence, as PostgreSQL models one.
///
/// Only PostgreSQL has sequences a person names and shares. SQLite and MySQL render no
/// statements for one and introspect none; the sequence behind an identity column is
/// [`Column::auto_increment`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    /// The sequence name, unqualified.
    pub name: String,
    /// The schema it lives in, or `None` for the default schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The data type of the values, as PostgreSQL spells it (`bigint`, `integer`, ...).
    #[serde(default = "default_sequence_type")]
    pub data_type: String,
    /// The value a fresh sequence starts at.
    #[serde(default = "default_one")]
    pub start: i64,
    /// How much each `nextval` advances the counter.
    #[serde(default = "default_one")]
    pub increment: i64,
    /// The lowest value the sequence will produce, or `None` for the data type's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    /// The highest value the sequence will produce, or `None` for the data type's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    /// Whether reaching the end wraps around instead of failing.
    #[serde(default)]
    pub cycle: bool,
    /// How many values each connection reserves at once.
    #[serde(default = "default_one")]
    pub cache: i64,
}

fn default_sequence_type() -> String {
    "bigint".into()
}

const fn default_one() -> i64 {
    1
}

impl core::default::Default for Sequence {
    fn default() -> Self {
        Self {
            name: String::new(),
            schema: None,
            data_type: default_sequence_type(),
            start: 1,
            increment: 1,
            min: None,
            max: None,
            cycle: false,
            cache: 1,
        }
    }
}

impl Sequence {
    /// A `bigint` sequence starting at 1, the way PostgreSQL creates one by default.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// The sequence's name, qualified by its schema when it has one.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }

    /// Fold a bound that equals the data type's default back to `None`.
    ///
    /// PostgreSQL always reports concrete bounds, while a declaration that does not care
    /// leaves them `None`. Folding both spellings together is what keeps a pushed sequence
    /// from being altered on every push.
    pub fn normalised(&self) -> Self {
        let (type_min, type_max) = type_bounds(&self.data_type);
        let (default_min, default_max) = if self.increment < 0 {
            (type_min, -1)
        } else {
            (1, type_max)
        };
        let mut normalised = self.clone();
        if normalised.min == Some(default_min) {
            normalised.min = None;
        }
        if normalised.max == Some(default_max) {
            normalised.max = None;
        }
        normalised
    }
}

/// The range a sequence's data type can hold.
fn type_bounds(data_type: &str) -> (i64, i64) {
    match data_type.to_ascii_lowercase().as_str() {
        "smallint" | "int2" => (i64::from(i16::MIN), i64::from(i16::MAX)),
        "integer" | "int" | "int4" => (i64::from(i32::MIN), i64::from(i32::MAX)),
        _ => (i64::MIN, i64::MAX),
    }
}

/// The command a row-level security policy applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PolicyCommand {
    /// Every command.
    #[default]
    All,
    /// `SELECT` only.
    Select,
    /// `INSERT` only.
    Insert,
    /// `UPDATE` only.
    Update,
    /// `DELETE` only.
    Delete,
}

impl PolicyCommand {
    /// The spelling `pg_policies.cmd` and `CREATE POLICY` use.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }

    /// Parse the catalogue's spelling.
    pub fn from_catalogue(value: &str) -> Self {
        match value.trim().to_ascii_uppercase().as_str() {
            "SELECT" => Self::Select,
            "INSERT" => Self::Insert,
            "UPDATE" => Self::Update,
            "DELETE" => Self::Delete,
            // `ALL` and anything PostgreSQL adds later mean the permissive default.
            _ => Self::All,
        }
    }
}

/// A PostgreSQL row-level security policy.
///
/// Policies are recorded, not managed: [`Schema::policies`] round-trips through `besu pull`
/// and back, but the differ ignores it, so a policy written in a migration is never dropped
/// by a `push` that does not know about it. Declaring one from the derive would need a
/// `CREATE POLICY` that can carry raw SQL expressions, which is a larger decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The table the policy applies to, qualified the same way a table is.
    pub table: String,
    /// The policy's name, unique per table.
    pub name: String,
    /// The command it applies to.
    #[serde(default)]
    pub command: PolicyCommand,
    /// The roles it applies to; an empty list means `PUBLIC`.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The `USING` expression that filters visible rows, as PostgreSQL reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub using: Option<String>,
    /// The `WITH CHECK` expression that validates new rows, as PostgreSQL reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_check: Option<String>,
}

/// A named enum type, as found in PostgreSQL.
///
/// The name is unqualified for an enum in the database's default schema and `schema.name`
/// otherwise, the same rule [`Table`] follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enum {
    /// The type name.
    pub name: String,
    /// The allowed values, in order.
    pub values: Vec<String>,
}

impl Schema {
    /// An empty schema.
    pub const fn new() -> Self {
        Self {
            tables: Vec::new(),
            enums: Vec::new(),
            views: Vec::new(),
            sequences: Vec::new(),
            policies: Vec::new(),
        }
    }

    /// Build a schema from the static metadata emitted by `#[derive(Table)]`.
    pub fn from_defs(defs: &[&'static TableDef]) -> Self {
        let mut schema = Self::new();
        for def in defs {
            schema.push_def(def);
        }
        schema.sort();
        schema
    }

    /// Add (or replace) a table from static metadata.
    pub fn push_def(&mut self, def: &TableDef) {
        let table = Table::from_def(def);
        if let Some(slot) = self
            .tables
            .iter_mut()
            .find(|t| t.schema == table.schema && t.name == table.name)
        {
            *slot = table;
        } else {
            self.tables.push(table);
        }
    }

    /// Find a table by name in the default schema.
    ///
    /// A table's identity is `(schema, name)`, so two tables called `users` in different
    /// schemas are distinct. This matches the unqualified table first; when there is no
    /// such table it falls back to the first table with that name, which keeps lookups by
    /// name working for schemas that were introspected explicitly. Use [`Schema::find`]
    /// when the schema matters.
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.find(None, name)
            .or_else(|| self.tables.iter().find(|t| t.name == name))
    }

    /// Find a table by name in the default schema, mutably.
    pub fn table_mut(&mut self, name: &str) -> Option<&mut Table> {
        if self
            .tables
            .iter()
            .any(|t| t.schema.is_none() && t.name == name)
        {
            return self
                .tables
                .iter_mut()
                .find(|t| t.schema.is_none() && t.name == name);
        }
        self.tables.iter_mut().find(|t| t.name == name)
    }

    /// Find a table by its schema and name.
    ///
    /// `schema` is the table's schema, or `None` for the default schema. This is the
    /// identity the differ uses.
    pub fn find(&self, schema: Option<&str>, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|t| t.name == name && t.schema.as_deref() == schema)
    }

    /// Find a table by a qualified name, as carried by [`Change`](crate::Change).
    ///
    /// The name is `schema.table` when the table has a schema, and a bare `table`
    /// otherwise.
    pub fn get(&self, qualified: &str) -> Option<&Table> {
        match qualified.split_once('.') {
            Some((schema, name)) => self.find(Some(schema), name),
            None => self.find(None, qualified),
        }
    }

    /// Find a named enum type by name.
    pub fn find_enum(&self, name: &str) -> Option<&Enum> {
        self.enums.iter().find(|e| e.name == name)
    }

    /// Find a view by its (possibly qualified) name.
    pub fn view(&self, name: &str) -> Option<&View> {
        self.views.iter().find(|view| view.name == name)
    }

    /// Declare a view, replacing one with the same name.
    pub fn add_view(&mut self, view: View) {
        match self
            .views
            .iter_mut()
            .find(|existing| existing.name == view.name)
        {
            Some(existing) => *existing = view,
            None => self.views.push(view),
        }
    }

    /// Find a sequence by its schema and name.
    pub fn find_sequence(&self, schema: Option<&str>, name: &str) -> Option<&Sequence> {
        self.sequences
            .iter()
            .find(|sequence| sequence.name == name && sequence.schema.as_deref() == schema)
    }

    /// Declare a sequence, replacing one with the same identity.
    pub fn add_sequence(&mut self, sequence: Sequence) {
        match self
            .sequences
            .iter_mut()
            .find(|existing| existing.name == sequence.name && existing.schema == sequence.schema)
        {
            Some(existing) => *existing = sequence,
            None => self.sequences.push(sequence),
        }
    }

    /// Sort tables by schema and name, enums, views, sequences and policies by name, so
    /// output is stable.
    pub fn sort(&mut self) {
        self.tables
            .sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.name.cmp(&b.name)));
        self.enums.sort_by(|a, b| a.name.cmp(&b.name));
        self.views.sort_by(|a, b| a.name.cmp(&b.name));
        self.sequences
            .sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.name.cmp(&b.name)));
        self.policies
            .sort_by(|a, b| a.table.cmp(&b.table).then_with(|| a.name.cmp(&b.name)));
        for table in &mut self.tables {
            table.normalise();
        }
    }

    /// Order views so that a view over another view comes after it.
    ///
    /// A view's dependencies are not recorded anywhere, so they are read out of the
    /// definition: an identifier after `FROM` or `JOIN` that names another view in this
    /// schema is a dependency. The match is exact and case-sensitive, and a dependency this
    /// cannot see — a view named inside a subquery in a way the token scan misses — simply
    /// keeps its alphabetical position. Cycles, which PostgreSQL rejects anyway, are broken
    /// arbitrarily.
    pub fn ordered_views(&self) -> Vec<&View> {
        fn visit<'a>(
            view: &'a View,
            schema: &'a Schema,
            seen: &mut Vec<&'a str>,
            out: &mut Vec<&'a View>,
        ) {
            if seen.contains(&view.name.as_str()) {
                return;
            }
            seen.push(&view.name);
            for dependency in view_dependencies(view) {
                if let Some(dependency) = schema.view(&dependency) {
                    visit(dependency, schema, seen, out);
                }
            }
            out.push(view);
        }

        let mut out = Vec::with_capacity(self.views.len());
        let mut seen = Vec::with_capacity(self.views.len());
        for view in &self.views {
            visit(view, self, &mut seen, &mut out);
        }
        out
    }

    /// Order tables so that a table always comes after the tables it references.
    ///
    /// Cycles (which are legal SQL when the constraint is added afterwards) are broken
    /// arbitrarily rather than reported as an error.
    pub fn topological_order(&self) -> Vec<&Table> {
        fn visit<'a>(
            table: &'a Table,
            schema: &'a Schema,
            seen: &mut Vec<&'a Table>,
            out: &mut Vec<&'a Table>,
        ) {
            if seen
                .iter()
                .any(|seen| seen.name == table.name && seen.schema == table.schema)
            {
                return;
            }
            seen.push(table);
            for fk in &table.foreign_keys {
                if fk.references_table == table.qualified_name() {
                    continue;
                }
                let referenced = schema
                    .get(&fk.references_table)
                    .or_else(|| schema.table(&fk.references_table));
                if let Some(dep) = referenced {
                    visit(dep, schema, seen, out);
                }
            }
            out.push(table);
        }

        let mut out = Vec::with_capacity(self.tables.len());
        let mut seen = Vec::with_capacity(self.tables.len());
        for table in &self.tables {
            visit(table, self, &mut seen, &mut out);
        }
        out
    }
}

/// A single table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// The SQL table name.
    pub name: String,
    /// An optional schema/namespace, meaningful to PostgreSQL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Columns in declaration order.
    pub columns: Vec<Column>,
    /// Secondary indexes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    /// Multi-column `UNIQUE` constraints.
    ///
    /// A single-column `UNIQUE` lives on the column itself, as [`Column::unique`]. A
    /// constraint over several columns has no per-column spelling and is a separate object
    /// the database owns, so it is kept here rather than being modelled as an index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uniques: Vec<Unique>,
    /// Foreign key constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    /// `CHECK` constraints, which the derive cannot declare and a snapshot can.
    ///
    /// A check is written inside `CREATE TABLE` and added or dropped with
    /// `ALTER TABLE ... ADD/DROP CONSTRAINT` on the dialects that support it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// Tables of named enums are rare; kept for fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Whether row-level security is enabled on this table.
    ///
    /// The differ only ever turns it **on**: a database with RLS enabled stays that way
    /// even if the declaration does not mention it, because disabling RLS silently removes
    /// the guarantee every policy on the table depends on.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rls: bool,
}

impl Table {
    /// Build from the static metadata emitted by the derive.
    pub fn from_def(def: &TableDef) -> Self {
        Self {
            name: def.name.into(),
            schema: def.schema.map(Into::into),
            columns: def.columns.iter().map(Column::from_def).collect(),
            indexes: def
                .indexes
                .iter()
                .map(|idx| Index {
                    name: idx.name.into(),
                    columns: idx.columns.iter().map(|c| (*c).into()).collect(),
                    unique: idx.unique,
                    kind: idx.kind,
                    // A `TableDef` has no way to spell a predicate, so a declared index is
                    // always a plain one.
                    predicate: None,
                })
                .collect(),
            // The derive has no attribute for a composite `UNIQUE`; a hand-written
            // `TableDef` cannot carry one either, so this is only ever populated by
            // introspection.
            uniques: Vec::new(),
            foreign_keys: def
                .foreign_keys
                .iter()
                .map(|fk| ForeignKey {
                    name: fk.name.into(),
                    columns: fk.columns.iter().map(|c| (*c).into()).collect(),
                    references_table: fk.references_table.into(),
                    references_columns: fk.references_columns.iter().map(|c| (*c).into()).collect(),
                    on_delete: fk.on_delete,
                    on_update: fk.on_update,
                })
                .collect(),
            // The derive has no attribute for a `CHECK` constraint, so this is only ever
            // populated by introspection or a hand-written snapshot.
            checks: Vec::new(),
            comment: None,
            rls: def.rls,
        }
    }

    /// The table's name, qualified by its schema when it has one, as in `app.users`.
    ///
    /// This is the identity [`Change`](crate::Change) carries and what
    /// [`Ddl::quote_table`](crate::Ddl::quote_table) expects; it is *not* quoted.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }

    /// Find a column by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The primary key columns, in declaration order.
    pub fn primary_key(&self) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }

    /// Whether this table has no explicit primary key.
    pub fn has_primary_key(&self) -> bool {
        self.columns.iter().any(|c| c.primary_key)
    }

    /// Sort columns so the primary key comes first and the rest keep their order.
    ///
    /// A stable order matters: diffing two schemas with different column order should not
    /// report changes, and this gives every producer of a [`Table`] a canonical layout.
    pub fn normalise(&mut self) {
        self.columns.sort_by_key(|c| !c.primary_key);
        self.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        self.uniques.sort_by(|a, b| a.name.cmp(&b.name));
        self.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
        self.checks.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// A multi-column `UNIQUE` constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unique {
    /// The constraint name.
    pub name: String,
    /// The constrained columns, in order.
    pub columns: Vec<String>,
}

/// A single column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// The SQL column name.
    pub name: String,
    /// The canonical type.
    pub ty: TypeDef,
    /// Whether the column accepts `NULL`.
    #[serde(default)]
    pub nullable: bool,
    /// Whether the database generates values for this column.
    #[serde(default)]
    pub auto_increment: bool,
    /// Whether the column is part of the primary key.
    #[serde(default)]
    pub primary_key: bool,
    /// Whether the column has its own `UNIQUE` constraint.
    #[serde(default)]
    pub unique: bool,
    /// The default, as a normalised SQL expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// How the database computes this column, if it is generated.
    ///
    /// A generated column is never written by an insert or an update; the IR carries the
    /// expression so DDL can render it inside `CREATE TABLE` and introspection can compare
    /// it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Generated>,
    /// An optional comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The column's previous name, if the declaration says it was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

impl Column {
    /// Build from the static metadata emitted by the derive.
    pub fn from_def(def: &ColumnDef) -> Self {
        Self {
            name: def.name.into(),
            ty: def.ty.clone(),
            // A generated column is not an identity column: the value comes from the
            // expression, not from a sequence, and rendering `GENERATED ... AS IDENTITY`
            // for it would be wrong. `ColumnDef` marks it as database-produced so the
            // derive and `set_model` skip it, and this is where the two meanings separate.
            auto_increment: def.auto_increment && def.generated.is_none(),
            nullable: def.nullable,
            primary_key: def.primary_key,
            unique: def.unique,
            default: def.default.map(default_to_sql),
            generated: def.generated.map(|generated| Generated {
                expression: generated.expression.into(),
                stored: generated.stored,
            }),
            comment: None,
            renamed_from: def.renamed_from.map(Into::into),
        }
    }

    /// Whether two columns describe the same thing, ignoring name.
    ///
    /// `normalise` folds types the dialect cannot tell apart, so a database that has a
    /// single integer type does not diff forever against a schema that distinguishes them.
    pub(crate) fn same_shape(&self, other: &Self, normalise: &dyn Fn(&TypeDef) -> TypeDef) -> bool {
        normalise(&self.ty) == normalise(&other.ty)
            && self.nullable == other.nullable
            && self.auto_increment == other.auto_increment
            && self.primary_key == other.primary_key
            && self.unique == other.unique
            && normalise_default(self.default.as_deref())
                == normalise_default(other.default.as_deref())
            && self.generated.as_ref().map(|generated| {
                (
                    normalise_generated_expression(&generated.expression),
                    generated.stored,
                )
            }) == other.generated.as_ref().map(|generated| {
                (
                    normalise_generated_expression(&generated.expression),
                    generated.stored,
                )
            })
    }
}

/// A column whose value the database computes from other columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generated {
    /// The SQL expression that computes the value.
    pub expression: String,
    /// Whether the value is stored on disk (`STORED`) or computed on read (`VIRTUAL`).
    pub stored: bool,
}

/// Fold the differences a database introduces into a generated column's expression.
///
/// PostgreSQL reports `lower(bio)` as `lower(bio)` but `to_tsvector('english', bio)` as
/// `to_tsvector('english'::regconfig, bio)`, and MySQL backticks every identifier. The
/// comparison therefore removes identifier quoting and `::type` casts, and folds
/// whitespace, so a declared expression converges against the one read back.
pub fn normalise_generated_expression(expression: &str) -> String {
    let mut out = String::with_capacity(expression.len());
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                // String literals are data: copy them untouched, including doubled quotes.
                out.push(ch);
                while let Some(inner) = chars.next() {
                    out.push(inner);
                    if inner == '\'' {
                        if chars.peek() == Some(&'\'') {
                            out.push(chars.next().unwrap_or('\''));
                        } else {
                            break;
                        }
                    }
                }
            }
            // A PostgreSQL cast adds no meaning to the comparison, and the parser adds one
            // for every argument that needs an implicit conversion.
            ':' if chars.peek() == Some(&':') => {
                chars.next();
                skip_type(&mut chars);
            }
            '"' | '`' => {}
            '\n' | '\r' | '\t' => out.push(' '),
            ch => out.push(ch),
        }
    }

    // Collapse the whitespace the cast removal may have left behind.
    let mut collapsed = String::with_capacity(out.len());
    let mut previous_space = false;
    for ch in out.chars() {
        if ch == ' ' {
            if !previous_space {
                collapsed.push(ch);
            }
            previous_space = true;
        } else {
            collapsed.push(ch);
            previous_space = false;
        }
    }

    crate::normalise_expression(collapsed.trim())
}

/// Consume the type name after a `::` cast.
fn skip_type(chars: &mut core::iter::Peekable<core::str::Chars<'_>>) {
    // A type name is words, dots and an optional `(precision, scale)` suffix.
    while let Some(ch) = chars.peek() {
        if ch.is_alphanumeric() || *ch == '_' || *ch == '.' || *ch == ' ' {
            chars.next();
        } else {
            break;
        }
    }
    if chars.peek() == Some(&'(') {
        let mut depth = 0usize;
        for ch in chars.by_ref() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    // Do not leave the space that separated the type from the next token.
    while chars.peek() == Some(&' ') {
        chars.next();
    }
}

/// A secondary index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// The index name.
    pub name: String,
    /// The indexed columns, in order.
    pub columns: Vec<String>,
    /// Whether the index is unique.
    #[serde(default)]
    pub unique: bool,
    /// The access method the index uses.
    ///
    /// Defaulted so a snapshot written before access methods existed loads as b-trees.
    #[serde(default, skip_serializing_if = "IndexKind::is_btree")]
    pub kind: IndexKind,
    /// The `WHERE` clause of a partial index, if it has one.
    ///
    /// Only PostgreSQL has this read back into the IR. SQLite supports partial indexes but
    /// its introspection skips them rather than guess, and MySQL has none, so both leave
    /// this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

/// A `CHECK` constraint.
///
/// The expression is a raw SQL boolean expression. It is compared after folding away the
/// redundant parentheses PostgreSQL adds, so a check declared as `value > 0` converges
/// against the `((value > 0))` the database reports back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// The constraint name.
    pub name: String,
    /// The boolean expression inside `CHECK (...)`.
    pub expression: String,
}

/// A foreign key constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// The constraint name.
    pub name: String,
    /// Columns on this table.
    pub columns: Vec<String>,
    /// The referenced table.
    pub references_table: String,
    /// The referenced columns.
    pub references_columns: Vec<String>,
    /// Behaviour on delete.
    #[serde(default)]
    pub on_delete: ReferenceAction,
    /// Behaviour on update.
    #[serde(default)]
    pub on_update: ReferenceAction,
}

pub use besu_core::schema::{IndexKind, ReferenceAction};

/// A default value, normalised to a SQL expression for comparison.
pub type Default = Option<String>;

fn default_to_sql(value: DefaultValue) -> String {
    value.render().into_owned()
}

/// Fold equivalent spellings of the same default together.
pub fn normalise_default(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_matches('\'').trim();
    let lowered = value.to_ascii_lowercase();
    let lowered = match lowered.as_str() {
        "now()" | "current_timestamp()" | "current_timestamp" => "current_timestamp",
        "" => return None,
        _ => return Some(lowered),
    };
    Some(lowered.into())
}

/// Fold equivalent spellings of a `CHECK` expression together.
///
/// PostgreSQL stores `CHECK (value > 0)` and reports it as `(value > 0)`, with one pair of
/// parentheses for every level it was written at. Stripping pairs that wrap the whole
/// expression leaves the spelling a person would write, so a check declared in a snapshot
/// converges instead of being dropped and added on every push.
///
/// Only parentheses wrapping the *whole* expression are removed; `(a > 0) AND (b > 0)` keeps
/// both of its pairs.
pub fn normalise_expression(expression: &str) -> String {
    let mut current = expression.trim();
    while let Some(inner) = strip_outer_parens(current) {
        current = inner.trim();
    }
    current.to_owned()
}

/// The inside of `sql` when one pair of parentheses wraps all of it.
fn strip_outer_parens(sql: &str) -> Option<&str> {
    let inner = sql.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    let mut quote = 0u8;
    for byte in inner.bytes() {
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = byte,
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                if depth == 0 {
                    // The opening parenthesis closed before the end, so the outer pair does
                    // not wrap the whole expression.
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    Some(inner)
}

#[cfg(feature = "fs")]
mod fs_impl {
    use super::Schema;
    use std::path::Path;

    impl Schema {
        /// Read a schema snapshot from a JSON file.
        pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
            let contents = std::fs::read_to_string(path)?;
            serde_json::from_str(&contents)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
        }

        /// Write this schema to a JSON snapshot file.
        pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
            let json = serde_json::to_string_pretty(self)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            std::fs::write(path, json)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IndexKind, PolicyCommand, Schema, Sequence, normalise_generated_expression,
        normalise_view_definition, view_body,
    };

    #[test]
    fn a_sqlite_view_definition_is_wrapped_and_a_postgres_one_is_not() {
        // SQLite reports the whole statement; PostgreSQL and MySQL report the body. The DDL
        // always needs the body, so the wrapper is stripped wherever it appears.
        assert_eq!(
            view_body(r#"CREATE VIEW "user_ids" AS SELECT id FROM users"#),
            "SELECT id FROM users"
        );
        assert_eq!(view_body("SELECT id FROM users"), "SELECT id FROM users");
    }

    #[test]
    fn equivalent_view_definitions_compare_equal() {
        // What PostgreSQL's `pg_get_viewdef` returns: qualified columns, re-indented, with a
        // trailing semicolon.
        assert_eq!(
            normalise_view_definition(
                " SELECT users.id,\n    users.name\n   FROM users\n  WHERE users.active;"
            ),
            normalise_view_definition("SELECT id, name FROM users WHERE active")
        );
        // MySQL's backticks and schema qualification.
        assert_eq!(
            normalise_view_definition("select `users`.`id` from `besu`.`users`"),
            normalise_view_definition("SELECT id FROM users")
        );
    }

    #[test]
    fn a_joined_view_keeps_its_qualifiers() {
        // With more than one table in play, `a.id` and `b.id` are different columns. Folding
        // the qualifier away would report a real change as no change.
        assert_ne!(
            normalise_view_definition("SELECT a.id FROM a JOIN b ON a.id = b.id"),
            normalise_view_definition("SELECT b.id FROM a JOIN b ON a.id = b.id")
        );
    }

    #[test]
    fn a_redundant_parenthesis_around_a_comparison_is_folded() {
        // PostgreSQL renders `WHERE id > 0` as `WHERE (id > 0)`.
        assert_eq!(
            normalise_view_definition("SELECT id FROM users WHERE (id > 0)"),
            normalise_view_definition("SELECT id FROM users WHERE id > 0")
        );
        // A pair that groups a boolean operator is not redundant, because removing it would
        // change the meaning.
        assert_ne!(
            normalise_view_definition("SELECT id FROM users WHERE (id > 0 OR id < -10)"),
            normalise_view_definition("SELECT id FROM users WHERE id > 0 OR id < -10 AND id < 0")
        );
    }

    #[test]
    fn a_different_view_definition_compares_different() {
        assert_ne!(
            normalise_view_definition("SELECT id FROM users WHERE active"),
            normalise_view_definition("SELECT id FROM users")
        );
    }

    #[test]
    fn a_generated_expression_folds_casts_and_quoting() {
        // PostgreSQL makes an implicit `regconfig` cast explicit; MySQL backticks names.
        assert_eq!(
            normalise_generated_expression("to_tsvector('english'::regconfig, bio)"),
            normalise_generated_expression("to_tsvector('english', bio)")
        );
        assert_eq!(
            normalise_generated_expression("(`price` * `quantity`)"),
            normalise_generated_expression("price * quantity")
        );
    }

    #[test]
    fn a_snapshot_without_an_index_kind_loads_as_a_b_tree() {
        // Access methods were added after the first snapshots were written, and an old one
        // must keep loading.
        let json = r#"{
            "tables": [{
                "name": "users",
                "columns": [{"name": "id", "ty": "Int8"}],
                "indexes": [
                    {"name": "users_id_idx", "columns": ["id"], "unique": false}
                ]
            }]
        }"#;
        let schema: Schema = serde_json::from_str(json).expect("an old snapshot must load");
        assert_eq!(schema.tables[0].indexes[0].kind, IndexKind::Btree);
    }

    #[test]
    fn sequence_defaults_are_the_postgres_ones() {
        let sequence = Sequence::new("order_number_seq");
        assert_eq!(sequence.data_type, "bigint");
        assert_eq!(sequence.start, 1);
        assert_eq!(sequence.increment, 1);
        assert_eq!(sequence.cache, 1);
        assert!(!sequence.cycle);
        assert_eq!(sequence.min, None);
        assert_eq!(sequence.max, None);

        let narrowed = Sequence {
            min: Some(1),
            max: Some(i64::MAX),
            ..Sequence::new("x")
        };
        assert_eq!(narrowed.normalised(), Sequence::new("x"));
    }

    #[test]
    fn a_policy_command_round_trips_through_the_catalogue_spelling() {
        for command in [
            PolicyCommand::All,
            PolicyCommand::Select,
            PolicyCommand::Insert,
            PolicyCommand::Update,
            PolicyCommand::Delete,
        ] {
            assert_eq!(PolicyCommand::from_catalogue(command.as_str()), command);
        }
    }
}
