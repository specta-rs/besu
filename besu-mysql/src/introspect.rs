use besu_core::{EnumDef, EnumLabels, TypeDef, schema::IndexKind, schema::ReferenceAction};
use besu_schema::{
    Check, Column, ForeignKey, Generated, Index, IndexColumn, Schema, Table, Unique, View,
};

use crate::MySql;

/// Read the database's schema from MySQL's `information_schema`.
///
/// The result is exactly what [`besu::Database::push`] compares against, so pushing a
/// schema that already exists produces no statements.
///
/// Functional (expression) indexes are read from `STATISTICS.EXPRESSION`. `CHECK`
/// constraints are read from `information_schema.CHECK_CONSTRAINTS` (MySQL 8.0.16+), with
/// MySQL's own backticks, keyword case and literal escaping folded back onto the declared
/// expression. Views are read into [`Schema::views`] for `besu pull`; MySQL has no
/// materialised views and no sequences. Triggers are not read at all.
///
/// The two drivers share this implementation: everything here is expressed as Besu SQL, so
/// the only requirement is a driver for [`MySql`] that can decode the columns
/// `information_schema` returns.
pub async fn introspect<Dr>(db: &besu::Database<Dr>) -> Result<Schema, Dr::Error>
where
    Dr: besu::Driver<Dialect = MySql>,
    String: besu_core::Decode<Dr>,
    u64: besu_core::Decode<Dr>,
    u32: besu_core::Decode<Dr>,
{
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

async fn introspect_views<Dr>(db: &besu::Database<Dr>) -> Result<Vec<View>, Dr::Error>
where
    Dr: besu::Driver<Dialect = MySql>,
    String: besu_core::Decode<Dr>,
{
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

async fn introspect_table<Dr>(db: &besu::Database<Dr>, name: String) -> Result<Table, Dr::Error>
where
    Dr: besu::Driver<Dialect = MySql>,
    String: besu_core::Decode<Dr>,
    u64: besu_core::Decode<Dr>,
    u32: besu_core::Decode<Dr>,
{
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

    // MySQL 8.0.16 and later keep a real table check in `CHECK_CONSTRAINTS`; the join to
    // `TABLE_CONSTRAINTS` is what scopes it to a table, because the check's own view does not
    // carry a table name.
    let checks = db
        .execute(table_query(
            "SELECT cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE \
             FROM information_schema.CHECK_CONSTRAINTS cc \
             JOIN information_schema.TABLE_CONSTRAINTS tc \
               ON tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA \
              AND tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
             WHERE cc.CONSTRAINT_SCHEMA = DATABASE() AND tc.TABLE_NAME = ? \
               AND tc.CONSTRAINT_TYPE = 'CHECK' \
             ORDER BY cc.CONSTRAINT_NAME",
            &name,
        ))
        .all::<(String, String)>()
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
                    // The default has to be rendered against the column's type: MySQL
                    // reports a literal string without the quotes the DDL needs back.
                    let default = parse_default(default.as_deref(), &extra, &ty);
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
                        default,
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
        checks: checks
            .into_iter()
            .map(|(name, expression)| Check {
                name,
                expression: normalise_check_expression(&expression),
            })
            .collect(),
        comment: None,
    };

    table.foreign_keys = group_foreign_keys(foreign_keys);

    // A spatial column's SRID is not part of `COLUMN_TYPE`; MySQL 8 keeps it in
    // `ST_GEOMETRY_COLUMNS`. A server that does not have the view (MariaDB, or a very old
    // MySQL) simply reports no SRIDs, which is the same as every column having none.
    let srids = db
        .execute(table_query(
            "SELECT COLUMN_NAME, SRS_ID FROM information_schema.ST_GEOMETRY_COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
            &name,
        ))
        .all::<(String, Option<u32>)>()
        .await
        .unwrap_or_default();
    for column in &mut table.columns {
        // The view reports an SRID of zero for a geometry with none, which is not an SRID
        // PostgreSQL or MySQL would name in a declaration.
        let srid = srids
            .iter()
            .find(|(column_name, _)| *column_name == column.name)
            .and_then(|(_, srid)| *srid)
            .filter(|srid| *srid != 0);
        match &mut column.ty {
            TypeDef::Geometry { srid: slot, .. } | TypeDef::Geography { srid: slot, .. } => {
                *slot = srid;
            }
            _ => {}
        }
    }

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
        // are not already indexed. Reporting it would make every push try to drop an index
        // the user never wrote. Its name is the constraint's when that was given, and the
        // first column's when MySQL generated the constraint name itself (`*_ibfk_*`).
        // The name has to be part of the test: when a user index already covers the
        // columns MySQL does not create one, and that index must still be reported.
        let auto_generated_constraint = format!("{}_ibfk_", table.name);
        if !index.unique
            && column_names.as_deref().is_some_and(|names| {
                table.foreign_keys.iter().any(|fk| {
                    fk.columns == names
                        && (fk.name == index.name
                            || (names.first().is_some_and(|first| *first == index.name)
                                && fk.name.starts_with(&auto_generated_constraint)))
                })
            })
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

/// Fold MySQL's spelling of a check expression onto the declared one.
///
/// `CHECK_CLAUSE` is what MySQL's own parser accepted, not a portable expression: every
/// identifier is backticked, a string literal carries a character-set introducer and
/// backslash-escaped quotes (`_latin1\'x\'`), and keywords are lowercased. Backticks and
/// keywords are folded by [`besu_schema::normalise_expression`]; the introducer and escapes
/// are MySQL's alone, so they are removed here.
fn normalise_check_expression(expression: &str) -> String {
    let mut unescaped = String::with_capacity(expression.len());
    let mut chars = expression.chars().peekable();

    while let Some(ch) = chars.next() {
        // MySQL escapes the opening quote too when it round-trips a check, so `\'x\'` and
        // `'x'` have to start the same literal.
        let starts_literal = ch == '\'' || (ch == '\\' && chars.peek() == Some(&'\''));
        if !starts_literal {
            unescaped.push(ch);
            continue;
        }
        if ch == '\\' {
            chars.next();
        }
        strip_introducer(&mut unescaped);
        unescaped.push('\'');
        while let Some(inner) = chars.next() {
            match inner {
                '\\' => {
                    // `\'` is a quote and `\\` a backslash; anything else keeps its
                    // backslash so the expression still looks like what was written.
                    match chars.next() {
                        Some(escaped @ ('\'' | '"' | '\\')) => unescaped.push(escaped),
                        Some(other) => {
                            unescaped.push('\\');
                            unescaped.push(other);
                        }
                        None => unescaped.push('\\'),
                    }
                }
                '\'' => {
                    unescaped.push(inner);
                    if chars.peek() == Some(&'\'') {
                        unescaped.push(chars.next().unwrap_or('\''));
                    } else {
                        break;
                    }
                }
                _ => unescaped.push(inner),
            }
        }
    }

    besu_schema::normalise_expression(&unescaped)
}

/// Remove a character-set introducer (`_latin1`, `_utf8mb4`, ...) directly before a literal.
fn strip_introducer(out: &mut String) {
    let start = out
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .map_or(0, |index| index + 1);
    let candidate = &out[start..];
    if candidate.len() > 1
        && candidate.starts_with('_')
        && candidate[1..].chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        out.truncate(start);
    }
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
        // An enum is inline, and `COLUMN_TYPE` holds its labels and nothing else: MySQL has
        // no type name to record, so the name is left empty.
        "enum" => match parse_enum(column_type) {
            Some(def) => TypeDef::Enum(def),
            None => TypeDef::Custom(column_type.to_owned().into()),
        },
        // A `SET` is inline too, and its labels are the whole type. The value is the
        // comma-separated list of chosen labels, which is the text MySQL sends.
        "set" => match parse_set(column_type) {
            Some(labels) => TypeDef::Set(labels),
            None => TypeDef::Custom(column_type.to_owned().into()),
        },
        // `BIT(n)` is a fixed-width bit string. MySQL reports the width in `COLUMN_TYPE`.
        "bit" => TypeDef::Bit(length(&lower).unwrap_or(1)),
        // MySQL's spatial types are named by their shape. The SRID is a column attribute in
        // `ST_GEOMETRY_COLUMNS`, which is read separately below; a column without one has no
        // SRID at all.
        "geometry" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::Geometry,
            srid: None,
        },
        "point" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::Point,
            srid: None,
        },
        "linestring" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::LineString,
            srid: None,
        },
        "polygon" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::Polygon,
            srid: None,
        },
        "multipoint" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::MultiPoint,
            srid: None,
        },
        "multilinestring" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::MultiLineString,
            srid: None,
        },
        "multipolygon" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::MultiPolygon,
            srid: None,
        },
        // MySQL 8 reports the collection as `geomcollection`; the standard name is also
        // accepted when the server writes it.
        "geomcollection" | "geometrycollection" => TypeDef::Geometry {
            kind: besu_core::GeometryKind::GeometryCollection,
            srid: None,
        },
        // Anything newer than this crate knows about keeps its native spelling, so generated
        // DDL round-trips.
        _ => TypeDef::Custom(column_type.to_owned().into()),
    }
}

/// Parse the labels out of MySQL's inline `enum('a','b')` spelling.
///
/// The keyword is matched case-insensitively, a label is whatever sits between single quotes
/// (with `''` as an escaped quote), and anything that does not match the shape exactly makes
/// the column a `Custom` type rather than a half-understood enum.
fn parse_enum(column_type: &str) -> Option<EnumDef> {
    Some(EnumDef {
        name: std::borrow::Cow::Borrowed(""),
        labels: parse_labels(column_type, "enum")?,
    })
}

/// Parse the labels out of MySQL's inline `set('a','b')` spelling.
fn parse_set(column_type: &str) -> Option<EnumLabels> {
    parse_labels(column_type, "set")
}

/// Parse the quoted label list after a keyword, or `None` when the shape does not match.
fn parse_labels(column_type: &str, keyword: &str) -> Option<EnumLabels> {
    let open = column_type.find('(')?;
    if !column_type[..open].trim().eq_ignore_ascii_case(keyword) {
        return None;
    }
    let close = column_type.rfind(')')?;
    if close < open || !column_type[close + 1..].trim().is_empty() {
        return None;
    }

    let mut chars = column_type[open + 1..close].chars().peekable();
    let mut labels = Vec::new();
    loop {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        if chars.next()? != '\'' {
            return None;
        }
        let mut label = String::new();
        loop {
            match chars.next()? {
                '\'' if chars.peek() == Some(&'\'') => {
                    chars.next();
                    label.push('\'');
                }
                '\'' => break,
                ch => label.push(ch),
            }
        }
        labels.push(label);

        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        match chars.peek() {
            None => break,
            Some(',') => {
                chars.next();
            }
            Some(_) => return None,
        }
    }

    Some(EnumLabels::Owned(labels))
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

/// Normalise a `COLUMN_DEFAULT` into a default Besu can execute.
///
/// `information_schema` reports a literal default as bare text — `hi` for `'hi'`,
/// `2024-01-02` for a date, `sad` for an enum label — so a snapshot that copies it verbatim
/// writes `DEFAULT hi`, which MySQL reads as an identifier. A literal on a string, temporal
/// or enum column is quoted here. An expression default is marked by `DEFAULT_GENERATED` in
/// `EXTRA` and comes back with MySQL's own escaping (`_latin1\'a\'`), which is turned back
/// into SQL text. A real SQL `NULL` arrives as `None`, because the column is read as an
/// `Option`; the text `NULL` is therefore the literal string.
fn parse_default(default: Option<&str>, extra: &str, ty: &TypeDef) -> Option<String> {
    let on_update = on_update_clause(extra);
    let generated = extra.to_ascii_lowercase().contains("default_generated");
    // The value is not trimmed: a literal default can legitimately start or end with a
    // space, and MySQL reports it exactly as it was written.
    let base = default.map(|default| {
        if generated {
            unescape_default_expression(default)
        } else if literal_default_needs_quotes(ty) {
            quote_default_literal(default)
        } else {
            default.to_owned()
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

/// Whether a literal default on this type has to be written as a quoted string.
///
/// Numeric, boolean, bit and binary defaults are already literals in their own spelling;
/// text, temporal and enum defaults are the bare values MySQL reports.
fn literal_default_needs_quotes(ty: &TypeDef) -> bool {
    match ty {
        TypeDef::Collated(inner, _) => literal_default_needs_quotes(inner),
        TypeDef::Domain(def) => literal_default_needs_quotes(def.base.as_type()),
        TypeDef::Char(_)
        | TypeDef::VarChar(_)
        | TypeDef::Text
        | TypeDef::Enum(_)
        | TypeDef::Set(_)
        | TypeDef::Date
        | TypeDef::Time
        | TypeDef::TimeP(_)
        | TypeDef::TimeTz
        | TypeDef::TimeTzP(_)
        | TypeDef::Timestamp
        | TypeDef::TimestampP(_)
        | TypeDef::TimestampTz
        | TypeDef::TimestampTzP(_) => true,
        _ => false,
    }
}

/// Quote a literal default and escape it the way MySQL's SQL parser expects.
fn quote_default_literal(value: &str) -> String {
    // A server that already returned quotes keeps the same value after they are removed.
    let value = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .unwrap_or(value);
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        match ch {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

/// Turn MySQL's escaped form of an expression default back into SQL text.
///
/// `information_schema` doubles the backslash before every quote (`_latin1\'a\'`), which is
/// not what a `DEFAULT (...)` clause accepts. The `DEFAULT_GENERATED` expression's own
/// parentheses are added by the DDL, not here.
fn unescape_default_expression(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('\'' | '"' | '\\') => out.push(chars.next().unwrap_or('\\')),
            // Any other escape is not part of the quoting layer; keep it intact.
            _ => out.push('\\'),
        }
    }
    out
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
    fn a_check_clause_with_a_literal_is_folded() {
        // `CHECK_CLAUSE` as MySQL reports it: backticks, a lowercased keyword, a charset
        // introducer and backslash-escaped quotes.
        assert_eq!(
            normalise_check_expression("((`name` <> _latin1\\'x\\') and (`balance` < 100))"),
            "name<>'x' and balance<100"
        );
    }

    #[test]
    fn an_inline_enum_parses_its_labels() {
        // `COLUMN_TYPE` is `enum('a','b')`, with the keyword lowercased by the server but a
        // label's case and escaped quotes preserved.
        assert_eq!(
            parse_type("enum", "enum('happy','it''s ok')"),
            TypeDef::Enum(EnumDef::owned("", ["happy", "it's ok"]))
        );
        assert_eq!(
            parse_type("enum", "ENUM('a', 'b')"),
            TypeDef::Enum(EnumDef::owned("", ["a", "b"]))
        );
        // Anything that does not match the shape stays a `Custom` type rather than becoming
        // an enum with labels Besu only half understood.
        assert_eq!(
            parse_type("enum", "enum(a,b)"),
            TypeDef::Custom("enum(a,b)".into())
        );
        assert_eq!(
            parse_type("set", "set('a','b')"),
            TypeDef::Set(EnumLabels::Owned(vec!["a".into(), "b".into()]))
        );
        assert_eq!(
            parse_type("set", "set(a,b)"),
            TypeDef::Custom("set(a,b)".into())
        );
    }

    #[test]
    fn a_literal_default_is_reported_as_valid_sql() {
        // MySQL reports a literal string as bare text, so it has to be quoted before it can
        // be written into a `DEFAULT` clause.
        assert_eq!(
            parse_default(Some("hi"), "", &TypeDef::VarChar(20)),
            Some("'hi'".into())
        );
        assert_eq!(
            parse_default(Some("it's"), "", &TypeDef::Text),
            Some("'it''s'".into())
        );
        // A literal's own leading and trailing spaces are part of its value.
        assert_eq!(
            parse_default(Some("  x  "), "", &TypeDef::VarChar(10)),
            Some("'  x  '".into())
        );
        assert_eq!(
            parse_default(Some("back\\slash"), "", &TypeDef::Text),
            Some("'back\\\\slash'".into())
        );
        // A literal that happens to spell `NULL` is not a SQL NULL; the latter arrives as
        // `None` from `information_schema`.
        assert_eq!(
            parse_default(Some("NULL"), "", &TypeDef::VarChar(20)),
            Some("'NULL'".into())
        );
        assert_eq!(parse_default(None, "", &TypeDef::VarChar(20)), None);
        // A temporal literal is unquoted in `COLUMN_DEFAULT`.
        assert_eq!(
            parse_default(Some("2024-01-02"), "", &TypeDef::Date),
            Some("'2024-01-02'".into())
        );
        assert_eq!(
            parse_default(
                Some("sad"),
                "",
                &TypeDef::Enum(EnumDef::owned("", ["happy", "sad"]))
            ),
            Some("'sad'".into())
        );
        // A numeric, bit or hex default is already a literal.
        for (default, ty) in [
            ("5", TypeDef::Int4),
            ("1.50", TypeDef::Decimal(5, 2)),
            ("b'1010'", TypeDef::Bit(4)),
            ("0x6162", TypeDef::Bytes),
            ("1", TypeDef::Bool),
        ] {
            assert_eq!(
                parse_default(Some(default), "", &ty),
                Some(default.to_owned()),
                "{ty:?}"
            );
        }
    }

    #[test]
    fn an_expression_default_is_unescaped_and_keeps_its_on_update_clause() {
        // `DEFAULT (concat('a','b'))` comes back with MySQL's escaped quoting.
        assert_eq!(
            parse_default(
                Some("concat(_latin1\\'a\\',_latin1\\'b\\')"),
                "DEFAULT_GENERATED",
                &TypeDef::VarChar(20)
            ),
            Some("concat(_latin1'a',_latin1'b')".into())
        );
        // `CURRENT_TIMESTAMP(6) ON UPDATE ...` is a keyword default, not an expression.
        assert_eq!(
            parse_default(
                Some("CURRENT_TIMESTAMP(6)"),
                "DEFAULT_GENERATED on update CURRENT_TIMESTAMP(6)",
                &TypeDef::TimestampP(6)
            ),
            Some("CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6)".into())
        );
    }

    #[test]
    fn bit_columns_keep_their_width() {
        assert_eq!(parse_type("bit", "bit(8)"), TypeDef::Bit(8));
        assert_eq!(parse_type("bit", "bit(1)"), TypeDef::Bit(1));
    }

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
