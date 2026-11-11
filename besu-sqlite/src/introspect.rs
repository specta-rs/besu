use besu_core::TypeDef;
use besu_core::schema::ReferenceAction;
use besu_schema::{Column, ForeignKey, Index, Schema, Table};

use crate::Sqlx;

/// Read the database's schema from SQLite's own catalogue.
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
    schema.sort();
    Ok(schema)
}

async fn introspect_table(
    db: &besu::Database<Sqlx>,
    name: String,
    create_sql: Option<String>,
) -> Result<Table, sqlx::Error> {
    let pragma = format!("PRAGMA table_info({})", quote_literal(&name));
    let columns = db
        .execute(besu::Fragment::new(pragma))
        .all::<(i64, String, String, i64, Option<String>, i64)>()
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
                |(_, column_name, ty, not_null, default, primary_key)| Column {
                    name: column_name,
                    ty: parse_type(&ty),
                    nullable: not_null == 0 && primary_key == 0,
                    auto_increment: auto_increment && primary_key != 0,
                    primary_key: primary_key != 0,
                    unique: false,
                    default: default.map(normalise_default),
                    comment: None,
                },
            )
            .collect(),
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
    };

    introspect_indexes(db, &mut table).await?;
    introspect_foreign_keys(db, &mut table).await?;
    Ok(table)
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

    for (_, name, unique, origin, _) in indexes {
        let info = format!("PRAGMA index_info({})", quote_literal(&name));
        let columns = db
            .execute(besu::Fragment::new(info))
            .all::<(i64, i64, Option<String>)>()
            .await?;
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
            // `PRIMARY KEY` automatic indexes.
            "pk" => {}
            _ => table.indexes.push(Index {
                name,
                columns,
                unique: unique != 0,
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
        "NUMERIC" | "DECIMAL" => TypeDef::Text,
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
