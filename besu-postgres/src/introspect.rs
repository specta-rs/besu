use std::collections::{BTreeMap, BTreeSet};

use besu_core::TypeDef;
use besu_core::schema::ReferenceAction;
use besu_schema::{Column, ForeignKey, Index, Schema, Table};

use crate::Sqlx;

/// Read the database's schema from PostgreSQL's own catalogue.
///
/// Only base tables in the connection's current schema are reported, and Besu's own
/// `_besu_migrations` bookkeeping table is skipped.
pub async fn introspect(db: &besu::Database<Sqlx>) -> Result<Schema, sqlx::Error> {
    let names = db
        .execute(besu::sql!(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = current_schema() \
               AND table_type = 'BASE TABLE' \
               AND table_name <> '_besu_migrations' \
             ORDER BY table_name"
        ))
        .all::<String>()
        .await?;

    let mut tables: BTreeMap<String, Table> = names
        .into_iter()
        .map(|name| {
            let table = Table {
                name: name.clone(),
                // Besu only ever looks at the current schema, so it is the implicit one.
                schema: None,
                columns: Vec::new(),
                indexes: Vec::new(),
                foreign_keys: Vec::new(),
                comment: None,
            };
            (name, table)
        })
        .collect();

    introspect_columns(db, &mut tables).await?;
    let constraints = introspect_constraints(db, &mut tables).await?;
    introspect_indexes(db, &mut tables, &constraints).await?;
    introspect_foreign_keys(db, &mut tables).await?;

    let mut schema = Schema::new();
    schema.tables = tables.into_values().collect();
    schema.sort();
    Ok(schema)
}

async fn introspect_columns(
    db: &besu::Database<Sqlx>,
    tables: &mut BTreeMap<String, Table>,
) -> Result<(), sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows = db
        .execute(besu::sql!(
            "SELECT table_name, column_name, data_type, udt_name, is_nullable, \
                    column_default, is_identity, character_maximum_length, \
                    numeric_precision, numeric_scale \
             FROM information_schema.columns \
             WHERE table_schema = current_schema() \
             ORDER BY table_name, ordinal_position"
        ))
        .all::<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            Option<i32>,
            Option<i32>,
            Option<i32>,
        )>()
        .await?;

    for (
        table_name,
        name,
        data_type,
        udt_name,
        is_nullable,
        default,
        is_identity,
        character_length,
        numeric_precision,
        numeric_scale,
    ) in rows
    {
        let Some(table) = tables.get_mut(&table_name) else {
            continue;
        };

        // A `SERIAL` column sequesters its sequence in the default, while a modern identity
        // column reports `is_identity`. Both mean "the database generates this value".
        let auto_increment = is_identity == "YES"
            || default
                .as_deref()
                .is_some_and(|default| default.starts_with("nextval("));

        table.columns.push(Column {
            name,
            ty: parse_type(
                &data_type,
                &udt_name,
                character_length,
                numeric_precision,
                numeric_scale,
            ),
            nullable: is_nullable == "YES",
            auto_increment,
            primary_key: false,
            unique: false,
            // An identity column has no default to round-trip: the sequence owns it.
            default: if auto_increment {
                None
            } else {
                default.as_deref().and_then(normalise_default)
            },
            comment: None,
            renamed_from: None,
        });
    }

    Ok(())
}

/// Read primary keys and `UNIQUE` constraints, returning every constraint name so their
/// backing indexes can be skipped.
async fn introspect_constraints(
    db: &besu::Database<Sqlx>,
    tables: &mut BTreeMap<String, Table>,
) -> Result<BTreeSet<String>, sqlx::Error> {
    let rows = db
        .execute(besu::sql!(
            "SELECT tc.table_name, tc.constraint_name, tc.constraint_type, kcu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON kcu.constraint_schema = tc.constraint_schema \
              AND kcu.constraint_name = tc.constraint_name \
             WHERE tc.table_schema = current_schema() \
               AND tc.constraint_type IN ('PRIMARY KEY', 'UNIQUE') \
             ORDER BY tc.table_name, tc.constraint_name, kcu.ordinal_position"
        ))
        .all::<(String, String, String, String)>()
        .await?;

    struct Constraint {
        table: String,
        name: String,
        kind: String,
        columns: Vec<String>,
    }

    let mut constraints: Vec<Constraint> = Vec::new();
    for (table, name, kind, column) in rows {
        match constraints
            .iter_mut()
            .find(|constraint| constraint.table == table && constraint.name == name)
        {
            Some(constraint) => constraint.columns.push(column),
            None => constraints.push(Constraint {
                table,
                name,
                kind,
                columns: vec![column],
            }),
        }
    }

    let mut names = BTreeSet::new();
    for constraint in constraints {
        names.insert(constraint.name.clone());

        let Some(table) = tables.get_mut(&constraint.table) else {
            continue;
        };

        if constraint.kind == "PRIMARY KEY" {
            for name in &constraint.columns {
                if let Some(column) = table.columns.iter_mut().find(|column| &column.name == name) {
                    column.primary_key = true;
                }
            }
        } else if let [column_name] = constraint.columns.as_slice() {
            // A single-column `UNIQUE` is also reported on the column, which is how the
            // derive declares it, so it round-trips instead of becoming an index.
            if let Some(column) = table
                .columns
                .iter_mut()
                .find(|column| &column.name == column_name)
            {
                column.unique = true;
            }
        } else {
            // A composite `UNIQUE` has no per-column spelling, so keep it as a unique index.
            // (PostgreSQL owns the index through the constraint, so dropping it needs a
            // migration that drops the constraint instead.)
            table.indexes.push(Index {
                name: constraint.name,
                columns: constraint.columns,
                unique: true,
            });
        }
    }

    Ok(names)
}

async fn introspect_indexes(
    db: &besu::Database<Sqlx>,
    tables: &mut BTreeMap<String, Table>,
    constraints: &BTreeSet<String>,
) -> Result<(), sqlx::Error> {
    let rows = db
        .execute(besu::sql!(
            "SELECT t.relname, i.relname, ix.indisunique, ix.indnkeyatts, a.attname \
             FROM pg_index ix \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(ix.indkey) \
             WHERE n.nspname = current_schema() \
               AND NOT ix.indisprimary \
               AND array_position(ix.indkey, a.attnum) <= ix.indnkeyatts \
             ORDER BY t.relname, i.relname, array_position(ix.indkey, a.attnum)"
        ))
        .all::<(String, String, bool, i16, String)>()
        .await?;

    struct PgIndex {
        table: String,
        name: String,
        unique: bool,
        keys: usize,
        columns: Vec<String>,
    }

    let mut indexes: Vec<PgIndex> = Vec::new();
    for (table, name, unique, keys, column) in rows {
        match indexes
            .iter_mut()
            .find(|index| index.table == table && index.name == name)
        {
            Some(index) => index.columns.push(column),
            None => indexes.push(PgIndex {
                table,
                name,
                unique,
                keys: usize::try_from(keys).unwrap_or(0),
                columns: vec![column],
            }),
        }
    }

    for index in indexes {
        // Indexes backing a primary key or unique constraint are already represented, and an
        // index on an expression has fewer columns than key attributes: skip both.
        if constraints.contains(&index.name) || index.columns.len() != index.keys {
            continue;
        }

        let Some(table) = tables.get_mut(&index.table) else {
            continue;
        };
        table.indexes.push(Index {
            name: index.name,
            columns: index.columns,
            unique: index.unique,
        });
    }

    Ok(())
}

async fn introspect_foreign_keys(
    db: &besu::Database<Sqlx>,
    tables: &mut BTreeMap<String, Table>,
) -> Result<(), sqlx::Error> {
    let rows = db
        .execute(besu::sql!(
            "SELECT t.relname, con.conname, rt.relname, con.confdeltype::text, \
                    con.confupdtype::text, a.attname, ra.attname \
             FROM pg_constraint con \
             JOIN pg_class t ON t.oid = con.conrelid \
             JOIN pg_class rt ON rt.oid = con.confrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) ON true \
             JOIN LATERAL unnest(con.confkey) WITH ORDINALITY AS rk(attnum, ord) \
               ON rk.ord = k.ord \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum \
             JOIN pg_attribute ra ON ra.attrelid = rt.oid AND ra.attnum = rk.attnum \
             WHERE con.contype = 'f' AND n.nspname = current_schema() \
             ORDER BY t.relname, con.conname, k.ord"
        ))
        .all::<(String, String, String, String, String, String, String)>()
        .await?;

    let mut foreign_keys: Vec<(String, ForeignKey)> = Vec::new();
    for (table, name, references_table, on_delete, on_update, column, references_column) in rows {
        if let Some((_, fk)) = foreign_keys
            .iter_mut()
            .find(|(existing, fk)| existing == &table && fk.name == name)
        {
            fk.columns.push(column);
            fk.references_columns.push(references_column);
            continue;
        }

        foreign_keys.push((
            table,
            ForeignKey {
                name,
                columns: vec![column],
                references_table,
                references_columns: vec![references_column],
                on_delete: parse_action(&on_delete),
                on_update: parse_action(&on_update),
            },
        ));
    }

    for (table, fk) in foreign_keys {
        if let Some(table) = tables.get_mut(&table) {
            table.foreign_keys.push(fk);
        }
    }

    Ok(())
}

/// Map a native PostgreSQL type back onto Besu's canonical types.
fn parse_type(
    data_type: &str,
    udt_name: &str,
    character_length: Option<i32>,
    numeric_precision: Option<i32>,
    numeric_scale: Option<i32>,
) -> TypeDef {
    match data_type {
        "boolean" => TypeDef::Bool,
        "smallint" => TypeDef::Int2,
        "integer" => TypeDef::Int4,
        "bigint" => TypeDef::Int8,
        "real" => TypeDef::Float4,
        "double precision" => TypeDef::Float8,
        "text" => TypeDef::Text,
        "character varying" => match character_length {
            Some(length) => TypeDef::VarChar(length_of(length)),
            // `VARCHAR` with no limit is `TEXT` with a different name.
            None => TypeDef::Text,
        },
        "character" => TypeDef::Char(length_of(character_length.unwrap_or(1))),
        "bytea" => TypeDef::Bytes,
        "uuid" => TypeDef::Uuid,
        "json" => TypeDef::Json,
        "jsonb" => TypeDef::Jsonb,
        "date" => TypeDef::Date,
        "time without time zone" => TypeDef::Time,
        "timestamp without time zone" => TypeDef::Timestamp,
        "timestamp with time zone" => TypeDef::TimestampTz,
        "numeric" | "decimal" => match (numeric_precision, numeric_scale) {
            // This is how `u64` is spelled, and there is no way to tell the two apart.
            (Some(20), Some(0)) => TypeDef::UInt(64),
            (Some(precision), Some(scale)) => TypeDef::Decimal(narrow(precision), narrow(scale)),
            (Some(precision), None) => TypeDef::Decimal(narrow(precision), 0),
            _ => TypeDef::Custom("numeric".into()),
        },
        "USER-DEFINED" | "ARRAY" => TypeDef::Custom(udt_name.to_owned().into()),
        other => TypeDef::Custom(other.to_owned().into()),
    }
}

fn narrow(value: i32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn length_of(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Map PostgreSQL's `confdeltype` / `confupdtype` codes onto an action.
///
/// `a` is `NO ACTION`, which is also what PostgreSQL reports when no action was written.
/// [`ReferenceAction::Default`] renders as no clause at all, so it round-trips to `NO ACTION`.
fn parse_action(code: &str) -> ReferenceAction {
    match code {
        "c" => ReferenceAction::Cascade,
        "n" => ReferenceAction::SetNull,
        "d" => ReferenceAction::SetDefault,
        "r" => ReferenceAction::Restrict,
        _ => ReferenceAction::Default,
    }
}

/// Normalise a `column_default` into the expression the derive would produce.
///
/// PostgreSQL stores defaults fully typed, as in `'oscar'::text` or `(0)::numeric`, so the
/// casts and wrapping parentheses are stripped to leave `'oscar'` and `0`.
fn normalise_default(value: &str) -> Option<String> {
    let mut normalised = value.trim().to_owned();

    while let Some(index) = trailing_cast(&normalised) {
        normalised.truncate(index);
        normalised = normalised.trim().to_owned();
    }

    loop {
        let trimmed = normalised.trim();
        match strip_parens(trimmed) {
            Some(inner) => normalised = inner.trim().to_owned(),
            None => {
                normalised = trimmed.to_owned();
                break;
            }
        }
    }

    if normalised.is_empty() {
        None
    } else {
        Some(normalised)
    }
}

/// The byte offset of a trailing `::type` cast, if there is one outside quotes and parentheses.
fn trailing_cast(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut quote = 0u8;
    let mut cast = None;
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if quote != 0 {
            if byte == quote {
                if bytes.get(index + 1) == Some(&quote) {
                    index += 1;
                } else {
                    quote = 0;
                }
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = byte,
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b':' if depth == 0 && bytes.get(index + 1) == Some(&b':') => {
                    cast = Some(index);
                    index += 1;
                }
                _ => {}
            }
        }
        index += 1;
    }

    let index = cast?;
    let suffix = &sql[index + 2..];
    let looks_like_type = !suffix.is_empty()
        && suffix.chars().all(|ch| {
            ch.is_alphanumeric() || matches!(ch, ' ' | '(' | ')' | '[' | ']' | '_' | '.' | '"')
        });
    looks_like_type.then_some(index)
}

/// Strip one pair of parentheses that wraps the whole expression.
fn strip_parens(sql: &str) -> Option<&str> {
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
                    // The opening parenthesis closes before the end, so the outer pair does
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
