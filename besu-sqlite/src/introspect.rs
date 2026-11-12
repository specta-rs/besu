use besu_core::TypeDef;
use besu_core::schema::ReferenceAction;
use besu_schema::{Column, ForeignKey, Index, Schema, Table, Unique, View};

use crate::Sqlx;

/// Read the database's schema from SQLite's own catalogue.
///
/// Objects the IR cannot express are skipped rather than reported as a lossy version of
/// themselves, because `push` would then try to drop or replace them:
///
/// - partial indexes, whose `WHERE` clause has no representation here,
/// - expression indexes, whose key parts are not columns,
/// - `CHECK` constraints, which would have to be parsed out of the `CREATE TABLE` text,
/// - triggers, which are not read at all.
///
/// Views are read into [`Schema::views`] so a `besu pull` snapshot names them, but the differ
/// ignores that list, so they never appear as tables and `push` never touches them.
///
/// A consequence of skipping checks is that a table rebuild (an `AlterColumn`) recreates the
/// table from the IR and therefore loses a hand-written check; see `docs/guide/migrations.md`.
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
        // SQLite's catalog has no view of a `CHECK` constraint: it exists only inside the
        // `CREATE TABLE` text. Parsing that text back into an expression is not something
        // the IR can promise to get right, so checks are left unmodelled here rather than
        // reported as a guess. The cost is that a table rebuild loses one.
        checks: Vec::new(),
        comment: None,
        rls: false,
    };

    introspect_indexes(db, &mut table).await?;
    introspect_foreign_keys(db, &mut table).await?;
    Ok(table)
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

        // A `NULL` column name marks an expression key part (`lower(email)`), which the IR
        // cannot express. Reporting the remaining columns would describe an index that does
        // not exist, so the whole index is skipped.
        if columns.iter().any(|(_, _, name)| name.is_none()) {
            continue;
        }
        let columns: Vec<String> = columns
            .into_iter()
            .filter_map(|(_, _, name)| name)
            .collect();

        if columns.is_empty() {
            continue;
        }

        match origin.as_str() {
            // A `UNIQUE` written on the column itself shows up as an automatic index.
            "u" if columns.len() == 1 && unique != 0 => {
                if let Some(column) = table.columns.iter_mut().find(|c| c.name == columns[0]) {
                    column.unique = true;
                }
            }
            // A multi-column `UNIQUE` is reported as an automatic index too. SQLite gives
            // that index a generated name (`sqlite_autoindex_<table>_<n>`) and does not
            // expose the constraint's own name, which is why the differ matches uniques by
            // their columns rather than by name.
            "u" if unique != 0 => table.uniques.push(Unique { name, columns }),
            // `PRIMARY KEY` automatic indexes.
            "pk" => {}
            _ => table.indexes.push(Index {
                name,
                columns,
                unique: unique != 0,
                // SQLite has one access method; everything it creates is a b-tree.
                kind: besu_schema::IndexKind::Btree,
                predicate: None,
            }),
        }
    }

    Ok(())
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
