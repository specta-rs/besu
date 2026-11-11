use besu_core::{TypeDef, schema::ReferenceAction};
use besu_schema::{Column, ForeignKey, Index, Schema, Table, Unique};

use crate::Sqlx;

/// Read the database's schema from MySQL's `information_schema`.
///
/// The result is exactly what [`besu::Database::push`] compares against, so pushing a
/// schema that already exists produces no statements.
pub async fn introspect(db: &besu::Database<Sqlx>) -> Result<Schema, sqlx::Error> {
    let tables = db
        .execute(besu::sql!(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
             AND LEFT(TABLE_NAME, 6) <> '_besu_' \
             ORDER BY TABLE_NAME"
        ))
        .all::<(String,)>()
        .await?;

    let mut schema = Schema::new();
    for (name,) in tables {
        schema.tables.push(introspect_table(db, name).await?);
    }
    schema.sort();
    Ok(schema)
}

/// A `SELECT` against `information_schema` filtered to one table.
fn table_query(sql: &'static str, table: &str) -> besu::Fragment {
    besu::Fragment::with_args(sql, vec![besu::Value::Text(table.to_owned())])
}

/// One index as reported by `information_schema.STATISTICS`, before it is classified.
struct RawIndex {
    name: String,
    columns: Vec<String>,
    unique: bool,
}

async fn introspect_table(db: &besu::Database<Sqlx>, name: String) -> Result<Table, sqlx::Error> {
    // `information_schema` reports numbers as `BIGINT UNSIGNED`, hence `u64` throughout.
    let columns = db
        .execute(table_query(
            "SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, \
             COLUMN_KEY \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
            &name,
        ))
        .all::<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        )>()
        .await?;

    let statistics = db
        .execute(table_query(
            "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND INDEX_NAME <> 'PRIMARY' \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            &name,
        ))
        .all::<(String, Option<String>, u64)>()
        .await?;

    let foreign_keys = db
        .execute(table_query(
            "SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_NAME, \
             k.REFERENCED_COLUMN_NAME, r.DELETE_RULE, r.UPDATE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
              AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
             WHERE k.TABLE_SCHEMA = DATABASE() AND k.TABLE_NAME = ? \
               AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
            &name,
        ))
        .all::<(String, String, String, String, String, String)>()
        .await?;

    let mut table = Table {
        name: name.clone(),
        schema: None,
        columns: columns
            .into_iter()
            .map(
                |(column_name, data_type, column_type, is_nullable, default, extra, column_key)| {
                    Column {
                        name: column_name,
                        ty: parse_type(&data_type, &column_type),
                        nullable: is_nullable.eq_ignore_ascii_case("YES"),
                        auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
                        primary_key: column_key.eq_ignore_ascii_case("PRI"),
                        // Filled in from `STATISTICS` below.
                        unique: false,
                        default: parse_default(default.as_deref(), &extra),
                        comment: None,
                        renamed_from: None,
                    }
                },
            )
            .collect(),
        indexes: Vec::new(),
        uniques: Vec::new(),
        foreign_keys: Vec::new(),
        comment: None,
    };

    table.foreign_keys = group_foreign_keys(foreign_keys);

    let mut indexes: Vec<RawIndex> = Vec::new();
    for (index_name, column_name, non_unique) in statistics {
        let Some(column_name) = column_name else {
            // A functional index has an expression instead of a column, which Besu cannot
            // model, so it is skipped rather than reported as a change every push.
            continue;
        };

        match indexes.iter_mut().find(|index| index.name == index_name) {
            Some(index) => index.columns.push(column_name),
            None => indexes.push(RawIndex {
                columns: vec![column_name],
                name: index_name,
                unique: non_unique == 0,
            }),
        }
    }

    // A single-column unique index is how MySQL stores a column's `UNIQUE` constraint, so
    // it becomes part of the column rather than a separate index. MySQL names that index
    // after the column, which is what distinguishes it from a unique index the user
    // declared themselves.
    for index in &indexes {
        if index.unique && index.columns.len() == 1 && index.name == index.columns[0] {
            if let Some(column) = table
                .columns
                .iter_mut()
                .find(|column| column.name == index.columns[0])
            {
                column.unique = true;
            }
        }
    }

    for index in indexes {
        if index.unique && index.columns.len() == 1 && index.name == index.columns[0] {
            continue;
        }

        // InnoDB silently creates an index for a foreign key when the referencing columns
        // are not already indexed, named after the constraint. Reporting it would make
        // every push try to drop an index the user never wrote.
        if !index.unique
            && table
                .foreign_keys
                .iter()
                .any(|fk| fk.columns == index.columns)
        {
            continue;
        }

        // MySQL has no constraint distinct from the unique key that enforces it, so a
        // multi-column unique key is the table's `Unique`. It is dropped with
        // `DROP CONSTRAINT` (8.0.19+) or `DROP INDEX`, both of which name that key.
        if index.unique && index.columns.len() > 1 {
            table.uniques.push(Unique {
                name: index.name,
                columns: index.columns,
            });
            continue;
        }

        table.indexes.push(Index {
            name: index.name,
            columns: index.columns,
            unique: index.unique,
        });
    }

    Ok(table)
}

fn group_foreign_keys(
    rows: Vec<(String, String, String, String, String, String)>,
) -> Vec<ForeignKey> {
    let mut foreign_keys: Vec<ForeignKey> = Vec::new();

    for (constraint, column, references_table, references_column, on_delete, on_update) in rows {
        match foreign_keys.iter_mut().find(|fk| fk.name == constraint) {
            Some(fk) => {
                fk.columns.push(column);
                fk.references_columns.push(references_column);
            }
            None => foreign_keys.push(ForeignKey {
                name: constraint,
                columns: vec![column],
                references_table,
                references_columns: vec![references_column],
                on_delete: parse_action(&on_delete),
                on_update: parse_action(&on_update),
            }),
        }
    }

    foreign_keys
}

fn parse_action(value: &str) -> ReferenceAction {
    match value.to_ascii_uppercase().as_str() {
        "CASCADE" => ReferenceAction::Cascade,
        "SET NULL" => ReferenceAction::SetNull,
        "SET DEFAULT" => ReferenceAction::SetDefault,
        "RESTRICT" => ReferenceAction::Restrict,
        // MySQL reports `NO ACTION` for an unspecified rule, which is the same as letting
        // the database decide.
        "NO ACTION" | "" => ReferenceAction::Default,
        _ => ReferenceAction::Default,
    }
}

/// Turn MySQL's native description of a column back into a canonical [`TypeDef`].
fn parse_type(data_type: &str, column_type: &str) -> TypeDef {
    let data_type = data_type.to_ascii_lowercase();
    let lower = column_type.to_ascii_lowercase();
    let unsigned = lower.contains("unsigned");

    match data_type.as_str() {
        // `BOOLEAN` is an alias for `TINYINT(1)`, which is what MySQL reports back.
        "bool" | "boolean" => TypeDef::Bool,
        "tinyint" if lower.starts_with("tinyint(1)") => TypeDef::Bool,
        "tinyint" if unsigned => TypeDef::UInt(8),
        "tinyint" => TypeDef::Int2,
        "smallint" if unsigned => TypeDef::UInt(16),
        "smallint" => TypeDef::Int2,
        "mediumint" if unsigned => TypeDef::UInt(32),
        "mediumint" => TypeDef::Int4,
        "int" | "integer" if unsigned => TypeDef::UInt(32),
        "int" | "integer" => TypeDef::Int4,
        "bigint" if unsigned => TypeDef::UInt(64),
        "bigint" => TypeDef::Int8,
        "float" => TypeDef::Float4,
        "double" | "real" => TypeDef::Float8,
        "char" => TypeDef::Char(length(&lower).unwrap_or(1)),
        "varchar" => match length(&lower) {
            Some(length) => TypeDef::VarChar(length),
            None => TypeDef::Text,
        },
        "tinytext" | "text" | "mediumtext" | "longtext" => TypeDef::Text,
        // A 16-byte `BINARY` is how this crate stores a UUID; everything else binary is
        // opaque bytes.
        "binary" if length(&lower) == Some(16) => TypeDef::Uuid,
        "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => TypeDef::Bytes,
        "json" => TypeDef::Json,
        "date" => TypeDef::Date,
        "time" => TypeDef::Time,
        "datetime" => TypeDef::Timestamp,
        "timestamp" => TypeDef::TimestampTz,
        "decimal" | "numeric" => {
            let (precision, scale) = decimal_parts(&lower).unwrap_or((10, 0));
            TypeDef::Decimal(precision, scale)
        }
        "year" => TypeDef::Int2,
        // Enums, sets and anything newer than this crate knows about keep their native
        // spelling, so generated DDL round-trips.
        _ => TypeDef::Custom(column_type.to_owned().into()),
    }
}

/// The first number inside `(...)`, as in `varchar(255)`.
fn length(column_type: &str) -> Option<u32> {
    let start = column_type.find('(')? + 1;
    let end = column_type[start..].find(')')? + start;
    column_type[start..end]
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// The precision and scale of `decimal(10,2)`.
fn decimal_parts(column_type: &str) -> Option<(u8, u8)> {
    let start = column_type.find('(')? + 1;
    let end = column_type[start..].find(')')? + start;
    let mut parts = column_type[start..end].split(',');
    let precision = parts.next()?.trim().parse().ok()?;
    let scale = parts.next().and_then(|part| part.trim().parse().ok());
    Some((precision, scale.unwrap_or(0)))
}

/// Normalise a `COLUMN_DEFAULT` into the expression Besu compares defaults with.
///
/// MySQL wraps string defaults in quotes and reports the `NULL` keyword as a plain string,
/// while an expression default comes back bare, marked by `DEFAULT_GENERATED` in `EXTRA`.
fn parse_default(default: Option<&str>, extra: &str) -> Option<String> {
    let default = default?.trim();

    if default.eq_ignore_ascii_case("NULL") {
        return None;
    }

    if extra.to_ascii_lowercase().contains("default_generated") {
        return Some(default.to_owned());
    }

    match default
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        Some(quoted) => Some(quoted.replace("''", "'")),
        None => Some(default.to_owned()),
    }
}
