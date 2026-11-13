use besu_core::TypeDef;
use besu_core::schema::ReferenceAction;
use besu_schema::{Check, Column, ForeignKey, Index, IndexColumn, Schema, Table, Unique, View};

use crate::Sqlx;

/// Read the database's schema from SQLite's own catalogue.
///
/// Objects the IR cannot express are skipped rather than reported as a lossy version of
/// themselves, because `push` would then try to drop or replace them:
///
/// - partial indexes, whose `WHERE` clause has no representation here,
/// - triggers, which are not read at all.
///
/// A `CHECK` constraint has no catalogue view either, so it is read out of the `CREATE TABLE`
/// text SQLite keeps in `sqlite_master`: the `CONSTRAINT name CHECK (...)` clause Besu renders
/// and any hand-written `CHECK (...)` are both recovered, so a check survives `push` and a
/// table rebuild.
///
/// An expression index is read from the `CREATE INDEX` text SQLite keeps in
/// `sqlite_master`, because `PRAGMA index_info` reports a `NULL` name for an expression key
/// part rather than the expression itself.
///
/// Views are read into [`Schema::views`] so a `besu pull` snapshot names them, but the differ
/// ignores that list, so they never appear as tables and `push` never touches them.
pub async fn introspect(db: &besu::Database<Sqlx>) -> Result<Schema, sqlx::Error> {
    let tables = db
        .execute(besu::sql!(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '\\_besu\\_%' ESCAPE '\\' \
             ORDER BY name"
        ))
        .all::<(String, Option<String>)>()
        .await?;

    let mut schema = Schema::new();
    for (name, create_sql) in tables {
        schema
            .tables
            .push(introspect_table(db, name, create_sql).await?);
    }
    schema.views = introspect_views(db).await?;
    schema.sort();
    Ok(schema)
}

async fn introspect_views(db: &besu::Database<Sqlx>) -> Result<Vec<View>, sqlx::Error> {
    let rows = db
        .execute(besu::sql!(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'view' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '\\_besu\\_%' ESCAPE '\\' \
             ORDER BY name"
        ))
        .all::<(String, Option<String>)>()
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(name, definition)| {
            Some(View {
                name,
                definition: definition?,
                materialised: false,
            })
        })
        .collect())
}

async fn introspect_table(
    db: &besu::Database<Sqlx>,
    name: String,
    create_sql: Option<String>,
) -> Result<Table, sqlx::Error> {
    // `table_xinfo` is `table_info` plus the `hidden` column, which is the only way to see
    // a generated column: `table_info` omits them entirely, exactly as if they did not
    // exist.
    let pragma = format!("PRAGMA table_xinfo({})", quote_literal(&name));
    let columns = db
        .execute(besu::Fragment::new(pragma))
        .all::<(i64, String, String, i64, Option<String>, i64, i64)>()
        .await?;

    let auto_increment = create_sql
        .as_deref()
        .is_some_and(|sql| sql.to_ascii_uppercase().contains("AUTOINCREMENT"));

    let mut table = Table {
        name: name.clone(),
        schema: None,
        columns: columns
            .into_iter()
            .map(
                |(_, column_name, ty, not_null, default, primary_key, hidden)| {
                    // `hidden` is 0 for an ordinary column, 1 for a hidden virtual-table
                    // column (never reported for a normal table), 2 for `VIRTUAL` and 3 for
                    // `STORED` generated columns.
                    let generated = match hidden {
                        2 | 3 => Some(besu_schema::Generated {
                            expression: create_sql
                                .as_deref()
                                .and_then(|sql| generated_expression(sql, &column_name))
                                .unwrap_or_default(),
                            stored: hidden == 3,
                        }),
                        _ => None,
                    };
                    Column {
                        name: column_name,
                        ty: parse_type(&ty),
                        nullable: not_null == 0 && primary_key == 0,
                        auto_increment: auto_increment && primary_key != 0,
                        primary_key: primary_key != 0,
                        unique: false,
                        default: default.map(normalise_default),
                        generated,
                        comment: None,
                        renamed_from: None,
                    }
                },
            )
            .collect(),
        indexes: Vec::new(),
        uniques: Vec::new(),
        foreign_keys: Vec::new(),
        checks: create_sql
            .as_deref()
            .map(|sql| check_constraints(sql, &name))
            .unwrap_or_default(),
        comment: None,
    };

    introspect_indexes(db, &mut table).await?;
    introspect_foreign_keys(db, &mut table).await?;
    Ok(table)
}

/// Read the `CHECK` constraints out of a `CREATE TABLE` statement.
///
/// SQLite's catalogue has no view of a check: it exists only in `sqlite_master.sql`. The
/// statement is therefore split into column definitions and table constraints at its
/// top-level commas, and every `CHECK (...)` in one is read, together with the `CONSTRAINT
/// name` that precedes it. A check written without a name cannot be named by the database, so
/// it is given the same `{table}_{i}_check` name the derive gives an unnamed one.
fn check_constraints(create_sql: &str, table: &str) -> Vec<Check> {
    let Some(open) = create_sql.find('(') else {
        return Vec::new();
    };
    let body = &create_sql[open + 1..];
    let mut checks = Vec::new();

    for segment in table_segments(body) {
        collect_checks(segment, &mut checks);
    }

    checks
        .into_iter()
        .enumerate()
        .map(|(position, (name, expression))| Check {
            name: name.unwrap_or_else(|| format!("{table}_{position}_check")),
            expression: besu_schema::normalise_expression(&expression),
        })
        .collect()
}

/// Split the inside of a `CREATE TABLE` body at its top-level commas.
fn table_segments(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                if bytes.get(index + 1) == Some(&open) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'[' => quote = Some(b']'),
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                segments.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start < body.len() {
        segments.push(&body[start..]);
    }
    segments
}

/// Push every top-level `CHECK (...)` in one table segment.
fn collect_checks(segment: &str, checks: &mut Vec<(Option<String>, String)>) {
    let bytes = segment.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                if bytes.get(index + 1) == Some(&open) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => {
                quote = Some(byte);
                index += 1;
                continue;
            }
            b'[' => {
                quote = Some(b']');
                index += 1;
                continue;
            }
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }

        if depth == 0
            && index + 5 <= bytes.len()
            && bytes[index..index + 5].eq_ignore_ascii_case(b"check")
        {
            let before_ok = index == 0 || !is_word_byte(bytes[index - 1]);
            let after_ok = index + 5 == bytes.len() || !is_word_byte(bytes[index + 5]);
            if before_ok && after_ok {
                let mut open = index + 5;
                while bytes.get(open).is_some_and(u8::is_ascii_whitespace) {
                    open += 1;
                }
                if bytes.get(open) == Some(&b'(')
                    && let Some((expression, end)) = parenthesised(segment, open)
                {
                    checks.push((
                        constraint_name_before(&segment[..index]),
                        expression.to_owned(),
                    ));
                    index = end;
                    continue;
                }
            }
        }

        index += 1;
    }
}

/// The inside of the parenthesised group starting at `open`, and the index after it.
fn parenthesised(segment: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = segment.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut index = open;

    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(close) = quote {
            if byte == close {
                if bytes.get(index + 1) == Some(&close) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'[' => quote = Some(b']'),
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((&segment[open + 1..index], index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

/// The name in a `CONSTRAINT name` immediately before a check, if one is written.
fn constraint_name_before(prefix: &str) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    let bytes = prefix.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b',') => {
                index += 1;
            }
            b'"' | b'`' => {
                let close = bytes[index];
                let start = index + 1;
                index = start;
                while index < bytes.len() {
                    if bytes[index] == close {
                        if bytes.get(index + 1) == Some(&close) {
                            index += 2;
                            continue;
                        }
                        break;
                    }
                    index += 1;
                }
                let raw = &prefix[start..index];
                let escaped = match close {
                    b'"' => raw.replace("\"\"", "\""),
                    b'`' => raw.replace("``", "`"),
                    _ => raw.to_owned(),
                };
                names.push(escaped);
                index += 1;
            }
            b'[' => {
                let start = index + 1;
                index = start;
                while index < bytes.len() && bytes[index] != b']' {
                    index += 1;
                }
                names.push(prefix[start..index].to_owned());
                index += 1;
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && !matches!(bytes[index], b'(' | b')' | b',')
                {
                    index += 1;
                }
                names.push(prefix[start..index].to_owned());
            }
        }
    }

    let [.., keyword, name] = names.as_slice() else {
        return None;
    };
    keyword
        .eq_ignore_ascii_case("constraint")
        .then(|| name.clone())
}

/// The expression of a generated column, read out of the `CREATE TABLE` text.
///
/// SQLite has no catalogue view of the expression: it exists only in `sqlite_master.sql`.
/// The definition of the wanted column is found by splitting the table body at top-level
/// commas, and the expression is the parenthesised group after its `GENERATED ALWAYS AS`.
fn generated_expression(create_sql: &str, column: &str) -> Option<String> {
    let start = create_sql.find('(')? + 1;
    let body = &create_sql[start..];
    let mut depth = 1usize;
    let mut quote = 0u8;
    let mut segment_start = 0usize;
    let mut segments: Vec<&str> = Vec::new();
    for (index, byte) in body.bytes().enumerate() {
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = byte,
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    segments.push(&body[segment_start..index]);
                    break;
                }
            }
            b',' if depth == 1 => {
                segments.push(&body[segment_start..index]);
                segment_start = index + 1;
            }
            _ => {}
        }
    }

    for segment in segments {
        let trimmed = segment.trim_start();
        // The first token of a column definition is its name, quoted or bare.
        let (name, rest) = if let Some(quoted) = trimmed.strip_prefix('"') {
            let (name, rest) = quoted.split_once('"')?;
            (name, rest)
        } else if let Some(quoted) = trimmed.strip_prefix('`') {
            let (name, rest) = quoted.split_once('`')?;
            (name, rest)
        } else {
            let end = trimmed
                .find(|ch: char| ch.is_whitespace() || ch == '(')
                .unwrap_or(trimmed.len());
            (&trimmed[..end], &trimmed[end..])
        };
        if !name.eq_ignore_ascii_case(column) {
            continue;
        }

        let lower = rest.to_ascii_lowercase();
        let bytes = lower.as_bytes();
        let mut as_at = None;
        for index in 0..bytes.len().saturating_sub(1) {
            if &bytes[index..index + 2] != b"as" {
                continue;
            }
            let previous_ok = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
            let next_ok = bytes
                .get(index + 2)
                .is_some_and(|next| next.is_ascii_whitespace() || *next == b'(');
            if previous_ok && next_ok {
                as_at = Some(index);
                break;
            }
        }
        let expression = rest[as_at? + 2..].trim_start();
        let expression = expression.strip_prefix('(')?;
        let mut depth = 1usize;
        let mut quote: Option<char> = None;
        for (index, ch) in expression.char_indices() {
            if let Some(open) = quote {
                if ch == open {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' | '`' => quote = Some(ch),
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(expression[..index].trim().to_owned());
                    }
                }
                _ => {}
            }
        }
        return None;
    }

    None
}

async fn introspect_indexes(
    db: &besu::Database<Sqlx>,
    table: &mut Table,
) -> Result<(), sqlx::Error> {
    let pragma = format!("PRAGMA index_list({})", quote_literal(&table.name));
    // (seq, name, unique, origin, partial)
    let indexes = db
        .execute(besu::Fragment::new(pragma))
        .all::<(i64, String, i64, String, i64)>()
        .await?;

    // The only place SQLite says what an expression key part is, is the `CREATE INDEX` text.
    // It is read once for the whole table rather than once per index.
    let definitions = db
        .execute(besu::Fragment::new(format!(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND tbl_name = {}",
            quote_literal(&table.name)
        )))
        .all::<(String, Option<String>)>()
        .await?;

    for (_, name, unique, origin, partial) in indexes {
        // SQLite knows partial indexes, but the IR has no predicate for one here, and a
        // partial index reported as a plain index would make `push` drop it or believe it
        // covers rows it does not.
        if partial != 0 {
            continue;
        }

        let info = format!("PRAGMA index_info({})", quote_literal(&name));
        let columns = db
            .execute(besu::Fragment::new(info))
            .all::<(i64, i64, Option<String>)>()
            .await?;

        if columns.is_empty() {
            continue;
        }

        let has_expression = columns.iter().any(|(_, _, name)| name.is_none());
        let definition = definitions
            .iter()
            .find(|(index, _)| index == &name)
            .and_then(|(_, sql)| sql.as_deref());

        // An expression key part has no column name in `index_info`; the key parts are read
        // from the statement instead. Without the statement there is no honest way to
        // describe the index, so it is skipped rather than reported as an index over its
        // plain columns only.
        let entries = if has_expression {
            let Some(parts) = definition.and_then(index_key_parts) else {
                continue;
            };
            if parts.len() != columns.len() {
                continue;
            }
            parts
                .into_iter()
                .zip(&columns)
                .map(|(part, (_, _, name))| match name {
                    Some(name) => IndexColumn::column(name.clone()),
                    None => IndexColumn::expression(part),
                })
                .collect::<Vec<_>>()
        } else {
            columns
                .iter()
                .filter_map(|(_, _, name)| name.clone())
                .map(IndexColumn::column)
                .collect::<Vec<_>>()
        };

        let column_names: Option<Vec<String>> = entries
            .iter()
            .map(|entry| entry.as_column().map(ToOwned::to_owned))
            .collect();

        match origin.as_str() {
            // A `UNIQUE` written on the column itself shows up as an automatic index.
            "u" if unique != 0 => match column_names.as_deref() {
                Some([column]) => {
                    if let Some(column) =
                        table.columns.iter_mut().find(|c| c.name == column.as_str())
                    {
                        column.unique = true;
                    }
                }
                // A multi-column `UNIQUE` is reported as an automatic index too. SQLite
                // gives that index a generated name (`sqlite_autoindex_<table>_<n>`) and
                // does not expose the constraint's own name, which is why the differ
                // matches uniques by their columns rather than by name.
                Some(columns) => table.uniques.push(Unique {
                    name,
                    columns: columns.to_vec(),
                }),
                // A unique expression index is not a `UNIQUE` constraint; keep it as an
                // index so it round-trips.
                None => table.indexes.push(Index {
                    name,
                    columns: entries,
                    unique: true,
                    kind: besu_schema::IndexKind::Btree,
                    predicate: None,
                }),
            },
            // `PRIMARY KEY` automatic indexes.
            "pk" => {}
            _ => table.indexes.push(Index {
                name,
                columns: entries,
                unique: unique != 0,
                // SQLite has one access method; everything it creates is a b-tree.
                kind: besu_schema::IndexKind::Btree,
                predicate: None,
            }),
        }
    }

    Ok(())
}

/// The key parts of a `CREATE INDEX` statement, in order.
///
/// SQLite has no catalogue view of an expression key part, so the statement is split at its
/// top-level commas. Quoted identifiers and string literals are respected, and nested
/// parentheses (a function call, for example) do not split an entry.
fn index_key_parts(sql: &str) -> Option<Vec<String>> {
    let on = keyword_position(sql, "on")?;
    let open = sql[on..].find('(')? + on;

    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = open + 1;
    let mut chars = sql[open..].char_indices().peekable();

    while let Some((offset, ch)) = chars.next() {
        let index = open + offset;
        if let Some(open_quote) = quote {
            if ch == open_quote {
                if chars.peek().is_some_and(|(_, next)| *next == open_quote) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let part = sql[start..index].trim();
                    if !part.is_empty() {
                        parts.push(part.to_owned());
                    }
                    return Some(parts);
                }
            }
            ',' if depth == 1 => {
                parts.push(sql[start..index].trim().to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }

    None
}

/// The byte offset of a bare keyword, ignoring text inside quotes.
///
/// `CREATE INDEX` text is whatever the caller wrote, so the `ON` can sit on its own line;
/// searching for the literal `" on "` would miss it.
fn keyword_position(sql: &str, keyword: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let needle = keyword.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                if bytes.get(index + 1) == Some(&open) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if index + needle.len() <= bytes.len()
            && bytes[index..index + needle.len()].eq_ignore_ascii_case(needle)
        {
            let before = index == 0 || !is_word_byte(bytes[index - 1]);
            let after =
                index + needle.len() == bytes.len() || !is_word_byte(bytes[index + needle.len()]);
            if before && after {
                return Some(index);
            }
        }
        index += 1;
    }

    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

async fn introspect_foreign_keys(
    db: &besu::Database<Sqlx>,
    table: &mut Table,
) -> Result<(), sqlx::Error> {
    let pragma = format!("PRAGMA foreign_key_list({})", quote_literal(&table.name));
    // (id, seq, table, from, to, on_update, on_delete, match)
    let rows = db
        .execute(besu::Fragment::new(pragma))
        .all::<(
            i64,
            i64,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
        )>()
        .await?;

    let mut current: Option<ForeignKey> = None;
    for (id, _, references_table, from, to, on_update, on_delete, _) in rows {
        let key = current.as_ref().map(|fk| fk.name.clone());
        let expected = format!("{}_{}_fkey", table.name, from);
        if key.as_deref() != Some(expected.as_str()) {
            if let Some(fk) = current.take() {
                table.foreign_keys.push(fk);
            }
            current = Some(ForeignKey {
                name: expected,
                columns: Vec::new(),
                references_table: references_table.clone(),
                references_columns: Vec::new(),
                on_delete: parse_action(&on_delete),
                on_update: parse_action(&on_update),
            });
        }
        if let Some(fk) = &mut current {
            fk.columns.push(from);
            fk.references_columns
                .push(to.unwrap_or_else(|| "id".into()));
        }
        let _ = id;
    }
    if let Some(fk) = current {
        table.foreign_keys.push(fk);
    }

    Ok(())
}

fn parse_action(value: &str) -> ReferenceAction {
    match value.to_ascii_uppercase().as_str() {
        "CASCADE" => ReferenceAction::Cascade,
        "SET NULL" => ReferenceAction::SetNull,
        "SET DEFAULT" => ReferenceAction::SetDefault,
        "RESTRICT" => ReferenceAction::Restrict,
        // `NO ACTION` is what SQLite reports for an unspecified action.
        "NO ACTION" | "" => ReferenceAction::Default,
        _ => ReferenceAction::Default,
    }
}

fn parse_type(declared: &str) -> TypeDef {
    let upper = declared.to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" => TypeDef::Int8,
        "REAL" => TypeDef::Float8,
        "TEXT" | "" => TypeDef::Text,
        "BLOB" => TypeDef::Bytes,
        "BOOLEAN" => TypeDef::Bool,
        "UUID" => TypeDef::Uuid,
        "JSON" => TypeDef::Json,
        "DATE" => TypeDef::Date,
        "TIME" => TypeDef::Time,
        "TIMESTAMP" | "DATETIME" => TypeDef::Timestamp,
        "NUMERIC" | "DECIMAL" => TypeDef::Decimal(0, 0),
        _ if upper.contains("INT") => TypeDef::Int8,
        _ if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") => {
            TypeDef::Text
        }
        _ if upper.contains("BLOB") => TypeDef::Bytes,
        _ if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") => {
            TypeDef::Float8
        }
        other => TypeDef::Custom(other.to_ascii_lowercase().into()),
    }
}

/// SQLite reports defaults verbatim, wrapped in parentheses.
fn normalise_default(value: String) -> String {
    let trimmed = value.trim();
    let trimmed = trimmed
        .strip_prefix('(')
        .and_then(|v| v.strip_suffix(')'))
        .unwrap_or(trimmed);
    trimmed.trim().to_owned()
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::{Check, check_constraints, index_key_parts};

    #[test]
    fn key_parts_split_on_top_level_commas() {
        assert_eq!(
            index_key_parts("CREATE INDEX x ON t (lower(email), kind DESC, (a + b))")
                .expect("the key list must parse"),
            ["lower(email)", "kind DESC", "(a + b)"]
        );
    }

    #[test]
    fn a_newline_before_on_does_not_hide_the_key_list() {
        assert_eq!(
            index_key_parts("CREATE INDEX x\nON t\n(lower(email))")
                .expect("the key list must parse"),
            ["lower(email)"]
        );
    }

    #[test]
    fn a_named_table_check_is_read_with_its_name() {
        assert_eq!(
            check_constraints(
                r#"CREATE TABLE "accounts" ("balance" INTEGER NOT NULL, CONSTRAINT "accounts_balance_check" CHECK (balance > 0))"#,
                "accounts",
            ),
            [Check {
                name: "accounts_balance_check".into(),
                expression: "balance>0".into(),
            }]
        );
    }

    #[test]
    fn a_column_check_without_a_name_is_given_the_derive_name() {
        assert_eq!(
            check_constraints(
                "CREATE TABLE accounts (\
                     id INTEGER PRIMARY KEY, \
                     starts_at TEXT NOT NULL, \
                     ends_at TEXT NOT NULL CHECK (ends_at IS NULL OR ends_at > starts_at), \
                     price INTEGER NOT NULL CHECK (price > 0))",
                "accounts",
            ),
            [
                Check {
                    name: "accounts_0_check".into(),
                    expression: "ends_at is null or ends_at>starts_at".into(),
                },
                Check {
                    name: "accounts_1_check".into(),
                    expression: "price>0".into(),
                },
            ]
        );
    }
}
