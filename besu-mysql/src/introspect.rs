use besu_core::{TypeDef, schema::IndexKind, schema::ReferenceAction};
use besu_schema::{Column, ForeignKey, Generated, Index, IndexColumn, Schema, Table, Unique, View};

use crate::Sqlx;

/// Read the database's schema from MySQL's `information_schema`.
///
/// The result is exactly what [`besu::Database::push`] compares against, so pushing a
/// schema that already exists produces no statements.
///
/// Functional (expression) indexes are read from `STATISTICS.EXPRESSION`. `CHECK`
/// constraints are skipped: MySQL reports them in a form it will not accept back (see the
/// field comment in `introspect_table`). Views are read into
/// [`Schema::views`] for `besu pull`; MySQL has no materialised views and no sequences.
/// Triggers are not read at all.
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
    schema.views = introspect_views(db).await?;
    schema.sort();
    Ok(schema)
}

async fn introspect_views(db: &besu::Database<Sqlx>) -> Result<Vec<View>, sqlx::Error> {
    let rows = db
        .execute(besu::sql!(
            "SELECT TABLE_NAME, VIEW_DEFINITION FROM information_schema.VIEWS \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME"
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

/// A `SELECT` against `information_schema` filtered to one table.
fn table_query(sql: &'static str, table: &str) -> besu::Fragment {
    besu::Fragment::with_args(sql, vec![besu::Value::Text(table.to_owned())])
}

/// One index as reported by `information_schema.STATISTICS`, before it is classified.
struct RawIndex {
    name: String,
    columns: Vec<IndexColumn>,
    unique: bool,
    /// The access method, or `None` when the IR has no name for it.
    kind: Option<IndexKind>,
}

/// The key parts as plain column names, when every part is one.
fn column_names(columns: &[IndexColumn]) -> Option<Vec<String>> {
    columns
        .iter()
        .map(|column| column.as_column().map(ToOwned::to_owned))
        .collect()
}

async fn introspect_table(db: &besu::Database<Sqlx>, name: String) -> Result<Table, sqlx::Error> {
    // `information_schema` reports numbers as `BIGINT UNSIGNED`, hence `u64` throughout.
    let columns = db
        .execute(table_query(
            "SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, \
             COLUMN_KEY, GENERATION_EXPRESSION, COLLATION_NAME \
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
            String,
            Option<String>,
        )>()
        .await?;

    // A per-column collation is only interesting when it differs from the table's: otherwise
    // every text column would carry the table's default and no declared `String` would ever
    // converge.
    let table_collation = db
        .execute(table_query(
            "SELECT TABLE_COLLATION FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
            &name,
        ))
        .all::<(Option<String>,)>()
        .await?
        .into_iter()
        .next()
        .and_then(|(collation,)| collation);

    let statistics = db
        .execute(table_query(
            "SELECT INDEX_NAME, COLUMN_NAME, EXPRESSION, NON_UNIQUE, INDEX_TYPE \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND INDEX_NAME <> 'PRIMARY' \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            &name,
        ))
        .all::<(String, Option<String>, Option<String>, u64, String)>()
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
                |(
                    column_name,
                    data_type,
                    column_type,
                    is_nullable,
                    default,
                    extra,
                    column_key,
                    generation_expression,
                    collation,
                )| {
                    let extra_lower = extra.to_ascii_lowercase();
                    // `EXTRA` is the only place MySQL says a column is generated; a
                    // `DEFAULT_GENERATED` marker means an expression default, not a
                    // generated column.
                    let stored = extra_lower.contains("stored generated");
                    let generated = (stored || extra_lower.contains("virtual generated"))
                        .then_some(generation_expression)
                        .filter(|expression| !expression.trim().is_empty())
                        .map(|expression| Generated {
                            expression: besu_schema::normalise_generated_expression(&expression),
                            stored,
                        });
                    let mut ty = parse_type(&data_type, &column_type);
                    if let Some(collation) = collation
                        && table_collation.as_deref() != Some(collation.as_str())
                    {
                        ty = TypeDef::Collated(Box::new(ty), collation.into());
                    }
                    Column {
                        name: column_name,
                        ty,
                        nullable: is_nullable.eq_ignore_ascii_case("YES"),
                        // A generated column owns its value, so it is not an
                        // `AUTO_INCREMENT` column.
                        auto_increment: extra_lower.contains("auto_increment"),
                        primary_key: column_key.eq_ignore_ascii_case("PRI"),
                        // Filled in from `STATISTICS` below.
                        unique: false,
                        default: parse_default(default.as_deref(), &extra),
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
        // MySQL stores a check as a `CHECK_CLAUSE` that is not the SQL it accepts back:
        // identifiers are backticked and string literals are backslash-escaped for MySQL's
        // own parser, so reporting one would make `besu pull` write a snapshot that cannot be
        // rendered again. Checks are therefore left unmodelled here, exactly as SQLite
        // leaves them: `push` neither sees nor drops one.
        checks: Vec::new(),
        comment: None,
    };

    table.foreign_keys = group_foreign_keys(foreign_keys);

    let mut indexes: Vec<RawIndex> = Vec::new();
    for (index_name, column_name, expression, non_unique, index_type) in statistics {
        // A column key part has a `COLUMN_NAME`; a functional one has an `EXPRESSION` and no
        // column. A prefix length is not part of the IR, so the column is reported whole.
        let entry = match (column_name, expression) {
            (Some(column_name), _) => Some(IndexColumn::column(column_name)),
            (None, Some(expression)) => Some(IndexColumn::expression(expression)),
            (None, None) => None,
        };
        let Some(entry) = entry else {
            continue;
        };
        let kind = parse_index_type(&index_type);
        match indexes.iter_mut().find(|index| index.name == index_name) {
            Some(index) => index.columns.push(entry),
            None => indexes.push(RawIndex {
                columns: vec![entry],
                name: index_name,
                unique: non_unique == 0,
                kind,
            }),
        }
    }

    // A single-column unique index is how MySQL stores a column's `UNIQUE` constraint, so
    // it becomes part of the column rather than a separate index. MySQL names that index
    // after the column, which is what distinguishes it from a unique index the user
    // declared themselves.
    for index in &indexes {
        if index.unique
            && let Some([column_name]) = column_names(&index.columns).as_deref()
            && index.name == column_name.as_str()
            && let Some(column) = table
                .columns
                .iter_mut()
                .find(|column| column.name == column_name.as_str())
        {
            column.unique = true;
        }
    }

    for index in indexes {
        // An access method the IR has no name for (`HASH`, which only the memory engine
        // uses) is skipped: reporting it as a b-tree would say an index exists that does
        // not.
        let Some(kind) = index.kind else {
            continue;
        };

        let column_names = column_names(&index.columns);

        if index.unique
            && column_names
                .as_deref()
                .is_some_and(|names| names.len() == 1 && index.name == names[0])
        {
            continue;
        }

        // InnoDB silently creates an index for a foreign key when the referencing columns
        // are not already indexed, named after the constraint. Reporting it would make
        // every push try to drop an index the user never wrote.
        if !index.unique
            && column_names
                .as_deref()
                .is_some_and(|names| table.foreign_keys.iter().any(|fk| fk.columns == names))
        {
            continue;
        }

        // MySQL has no constraint distinct from the unique key that enforces it, so a
        // multi-column unique key is the table's `Unique`. It is dropped with
        // `DROP CONSTRAINT` (8.0.19+) or `DROP INDEX`, both of which name that key. A
        // unique functional key is not a column list and stays an index.
        if index.unique
            && let Some(columns) = column_names
            && columns.len() > 1
        {
            table.uniques.push(Unique {
                name: index.name,
                columns,
            });
            continue;
        }

        table.indexes.push(Index {
            name: index.name,
            columns: index.columns,
            unique: index.unique,
            kind,
            // MySQL has no partial indexes.
            predicate: None,
        });
    }

    Ok(table)
}

/// Map `information_schema.STATISTICS.INDEX_TYPE` onto an [`IndexKind`].
///
/// `None` means the IR has no name for the method, so the index is skipped rather than
/// reported as a plain b-tree.
fn parse_index_type(index_type: &str) -> Option<IndexKind> {
    match index_type.to_ascii_uppercase().as_str() {
        "BTREE" => Some(IndexKind::Btree),
        "FULLTEXT" => Some(IndexKind::FullText),
        "SPATIAL" => Some(IndexKind::Spatial),
        // `RTREE` is the storage engine's own name for a spatial index.
        "RTREE" => Some(IndexKind::Spatial),
        _ => None,
    }
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
        // `BOOLEAN` is an alias for `TINYINT(1)`, which is what MySQL reports back. The
        // `unsigned` test comes first: a legacy `tinyint(1) unsigned` is a `u8` column, not a
        // boolean, and matching the width first would decode it as one.
        "bool" | "boolean" => TypeDef::Bool,
        "tinyint" if unsigned => TypeDef::UInt(8),
        "tinyint" if lower.starts_with("tinyint(1)") => TypeDef::Bool,
        // A signed `TINYINT` is a real storage width that `SMALLINT` would not recreate, so
        // the native name is kept and `normalise_type` folds it onto `Int2`.
        "tinyint" => TypeDef::Custom(lower.into()),
        "smallint" if unsigned => TypeDef::UInt(16),
        "smallint" => TypeDef::Int2,
        "mediumint" if unsigned => TypeDef::UInt(32),
        // See the `tinyint` arm: `INT` is wider than the column really is.
        "mediumint" => TypeDef::Custom(lower.into()),
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
        "text" => TypeDef::Text,
        // `TINYTEXT`, `MEDIUMTEXT` and `LONGTEXT` have different storage limits, which
        // `TEXT` would silently change; the exact name is kept.
        "tinytext" | "mediumtext" | "longtext" => TypeDef::Custom(lower.into()),
        // A 16-byte `BINARY` is how this crate stores a UUID; everything else binary is
        // opaque bytes.
        "binary" if length(&lower) == Some(16) => TypeDef::Uuid,
        "binary" | "varbinary" | "blob" => TypeDef::Bytes,
        "tinyblob" | "mediumblob" | "longblob" => TypeDef::Custom(lower.into()),
        "json" => TypeDef::Json,
        "date" => TypeDef::Date,
        // MySQL's `TIME`, `DATETIME` and `TIMESTAMP` carry a fractional-second precision,
        // and dropping it truncates the nanoseconds a `Value::Time` holds.
        "time" => match length(&lower) {
            Some(precision) => TypeDef::TimeP(narrow(precision)),
            None => TypeDef::Time,
        },
        "datetime" => match length(&lower) {
            Some(precision) => TypeDef::TimestampP(narrow(precision)),
            None => TypeDef::Timestamp,
        },
        "timestamp" => match length(&lower) {
            Some(precision) => TypeDef::TimestampTzP(narrow(precision)),
            None => TypeDef::TimestampTz,
        },
        "decimal" | "numeric" => {
            let (precision, scale) = decimal_parts(&lower).unwrap_or((10, 0));
            TypeDef::Decimal(precision, scale)
        }
        // A `YEAR` column has its own range and formatting; `SMALLINT` is not the same type.
        "year" => TypeDef::Custom(lower.into()),
        // Enums, sets and anything newer than this crate knows about keep their native
        // spelling, so generated DDL round-trips.
        _ => TypeDef::Custom(column_type.to_owned().into()),
    }
}

/// Narrow a parsed precision to the `u8` a [`TypeDef`] carries.
fn narrow(precision: u32) -> u8 {
    u8::try_from(precision).unwrap_or(u8::MAX)
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
    let on_update = on_update_clause(extra);
    let base = default.map(str::trim).and_then(|default| {
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
    });

    // `ON UPDATE` is an attribute of the column rather than a default, but MySQL's `EXTRA`
    // gives it no other home in the IR: appending it to the default means the DDL renders
    // `DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP`, which is the exact syntax the
    // server accepts back.
    match (base, on_update) {
        (Some(default), Some(clause)) => Some(format!("{default} {clause}")),
        // Without a default there is nothing Besu can attach the clause to, so an
        // `ON UPDATE`-only column is reported without it rather than with a wrong default.
        (None, _) => None,
        (Some(default), None) => Some(default),
    }
}

/// The `ON UPDATE ...` clause in MySQL's `EXTRA`, as SQL.
fn on_update_clause(extra: &str) -> Option<String> {
    let lower = extra.to_ascii_lowercase();
    let at = lower.find("on update ")?;
    let clause = extra[at + "on update ".len()..].trim();
    if clause.is_empty() {
        None
    } else {
        Some(format!("ON UPDATE {clause}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tinyint_one_unsigned_is_a_byte_not_a_boolean() {
        // `BOOLEAN` is an alias for `TINYINT(1)`, so the width alone cannot decide. The
        // `unsigned` test has to come first or a legacy `u8` column decodes as `bool`.
        assert_eq!(parse_type("tinyint", "tinyint(1)"), TypeDef::Bool);
        assert_eq!(
            parse_type("tinyint", "tinyint(1) unsigned"),
            TypeDef::UInt(8)
        );
        // A signed `TINYINT` keeps its native width, which is narrower than `SMALLINT`.
        assert_eq!(
            parse_type("tinyint", "tinyint(4)"),
            TypeDef::Custom("tinyint(4)".into())
        );
        assert_eq!(
            parse_type("tinyint", "tinyint"),
            TypeDef::Custom("tinyint".into())
        );
    }
}
