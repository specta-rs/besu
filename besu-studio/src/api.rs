use std::time::Instant;

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use besu::{
    Database, Decode, Dialect, Driver, Fragment, Sql, TypeDef, Value as SqlValue,
    schema::{Column, Schema, Table, migrate::split_statements},
};
use besu_any::AnyDatabase;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::AppState;

/// The page size when a request does not ask for one.
const DEFAULT_LIMIT: u32 = 50;

/// The most rows a single page may ask for.
const MAX_LIMIT: u32 = 500;

/// The most rows the SQL console returns. A hand-written query can be much wider than a
/// table page, and the grid is not built to scroll a million cells.
const QUERY_LIMIT: u32 = 500;

/// What the UI shows about the connection.
pub(crate) async fn info(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "dialect": state.info.dialect,
        "database": database_label(&state.info.connection),
        "version": state.info.version,
        // The studio has no read-only mode: the warning in the crate docs says so, and the
        // field exists so the UI can say it too without guessing.
        "read_only": false,
    }))
}

/// The introspected schema, serialised as-is.
pub(crate) async fn schema(State(state): State<AppState>) -> Result<Json<Schema>, ApiError> {
    state
        .db
        .introspect()
        .await
        .map(Json)
        .map_err(ApiError::backend)
}

/// The readable name of the database a connection string points at.
///
/// A URL names its database in its last non-empty path segment, so
/// `sqlite:///tmp/app.db` is `app.db` and `postgres://user@host/app` is `app`. A DSN such
/// as `host=… dbname=app` has no path to read, so the whole (already redacted) string is
/// shown; that is still more useful than nothing.
fn database_label(connection: &str) -> &str {
    let Some((_, rest)) = connection.split_once("://") else {
        return connection;
    };

    // A password or an SSL option may contain a slash, so drop the query before splitting.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);

    let Some(segment) = rest.rsplit('/').find(|segment| !segment.is_empty()) else {
        return connection;
    };

    // `postgres://user@host` has no path at all, and the authority is not a database name.
    let segment = match segment.rsplit_once('@') {
        Some((_, host)) => host,
        None => segment,
    };

    if segment.is_empty() {
        connection
    } else {
        segment
    }
}

/// What `GET /api/tables/{table}/rows` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct RowQuery {
    /// How many rows to return, capped at [`MAX_LIMIT`].
    limit: Option<u32>,
    /// How many rows to skip.
    offset: Option<u32>,
    /// `column:asc,other:desc`, in order.
    sort: Option<String>,
    /// A JSON array of [`Filter`] objects.
    filter: Option<String>,
}

/// One sort clause from the `sort` parameter.
#[derive(Debug)]
struct Sort {
    column: String,
    direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Asc,
    Desc,
}

/// One filter from the `filter` parameter.
#[derive(Debug, Deserialize)]
struct Filter {
    column: String,
    op: FilterOp,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FilterOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Contains,
    StartsWith,
    IsNull,
    IsNotNull,
}

impl FilterOp {
    fn name(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Neq => "neq",
            Self::Gt => "gt",
            Self::Gte => "gte",
            Self::Lt => "lt",
            Self::Lte => "lte",
            Self::Contains => "contains",
            Self::StartsWith => "starts_with",
            Self::IsNull => "is_null",
            Self::IsNotNull => "is_not_null",
        }
    }

    /// Whether this filter compares against a value rather than testing for `NULL`.
    fn needs_value(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }

    /// The SQL operator for an ordering comparison, if this is one.
    fn comparison(self) -> Option<&'static str> {
        match self {
            Self::Eq => Some(" = "),
            Self::Neq => Some(" <> "),
            Self::Gt => Some(" > "),
            Self::Gte => Some(" >= "),
            Self::Lt => Some(" < "),
            Self::Lte => Some(" <= "),
            Self::Contains | Self::StartsWith | Self::IsNull | Self::IsNotNull => None,
        }
    }
}

/// The table or view a request is about, with the columns the UI will display.
///
/// Columns are cloned out of the schema because a view that introspection does not describe
/// has to be probed, and then the columns are synthesised rather than borrowed.
struct Target {
    schema: Option<String>,
    name: String,
    columns: Vec<Column>,
    primary_key: Vec<String>,
}

impl Target {
    fn table(table: &Table) -> Self {
        Self {
            schema: table.schema.clone(),
            name: table.name.clone(),
            columns: table.columns.clone(),
            primary_key: table
                .primary_key()
                .into_iter()
                .map(|column| column.name.clone())
                .collect(),
        }
    }

    /// A view whose columns were learned from the database rather than the schema.
    ///
    /// Every type is [`TypeDef::Text`] because a `SELECT *` probe reports names, not types,
    /// and cells are stringified anyway. The alternative — refusing to show the view — is
    /// worse for a development tool.
    fn synthetic(schema: Option<String>, name: String, columns: Vec<String>) -> Self {
        Self {
            schema,
            name,
            columns: columns.iter().map(|name| text_column(name)).collect(),
            primary_key: Vec::new(),
        }
    }

    fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A column a view probe found, described as text.
fn text_column(name: &str) -> Column {
    Column {
        name: name.to_owned(),
        ty: TypeDef::Text,
        nullable: true,
        auto_increment: false,
        primary_key: false,
        unique: false,
        default: None,
        generated: None,
        comment: None,
        renamed_from: None,
    }
}

/// A page of rows, a row write, and everything either one needs to find its table.
///
/// The name is resolved against the schema before any SQL exists. It never reaches a
/// statement verbatim: every identifier goes through [`Sql::push_table`] or
/// [`Sql::push_ident`], which quote it the dialect's own way.
pub(crate) async fn table_rows(
    State(state): State<AppState>,
    Path(name): Path<String>,
    page: Result<Query<RowQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let started = Instant::now();
    let Query(page) = page.map_err(|err| ApiError::bad_request(err.to_string()))?;

    let limit = page.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = page.offset.unwrap_or(0);

    let schema = state.db.introspect().await.map_err(ApiError::backend)?;
    let target = resolve_target(&name, &schema, &state.db).await?;
    let sorts = parse_sort(page.sort.as_deref(), &target)?;
    let filters = parse_filters(page.filter.as_deref())?;
    validate_filters(&filters, &target)?;

    let total = match &state.db {
        AnyDatabase::Sqlite(db) => count_rows(db, &target, &filters).await,
        AnyDatabase::Postgres(db) => count_rows(db, &target, &filters).await,
        AnyDatabase::MySql(db) => count_rows(db, &target, &filters).await,
    }?;

    let rows = match &state.db {
        AnyDatabase::Sqlite(db) => read_rows(db, &target, &sorts, &filters, limit, offset).await,
        AnyDatabase::Postgres(db) => read_rows(db, &target, &sorts, &filters, limit, offset).await,
        AnyDatabase::MySql(db) => read_rows(db, &target, &sorts, &filters, limit, offset).await,
    }?;

    Ok(Json(json!({
        "table": target.qualified_name(),
        "columns": target.columns,
        "rows": rows,
        "total": total,
        "limit": limit,
        "offset": offset,
        "primary_key": target.primary_key,
        "duration_ms": elapsed_ms(started),
    })))
}

/// Find a table first, then a view, and describe whichever one exists.
async fn resolve_target(name: &str, schema: &Schema, db: &AnyDatabase) -> Result<Target, ApiError> {
    if let Some(table) = schema.get(name).or_else(|| schema.table(name)) {
        return Ok(Target::table(table));
    }

    let Some(view) = schema.view(name) else {
        return Err(ApiError::not_found(format!(
            "no table or view named `{name}` in the database"
        )));
    };

    // PostgreSQL reports a view's columns as a table too, so it has already returned above.
    // SQLite and MySQL list a view only under `Schema::views`, which carries no columns; a
    // `SELECT *` probe is the only thing that knows them. An empty view cannot answer, so
    // its columns stay unknown and the UI shows no grid for it.
    let (schema_name, view_name) = split_qualified(&view.name);
    let columns = view_columns(db, schema_name, view_name).await?;

    Ok(Target::synthetic(
        schema_name.map(str::to_owned),
        view_name.to_owned(),
        columns,
    ))
}

/// Split a possibly schema-qualified name into its parts.
fn split_qualified(name: &str) -> (Option<&str>, &str) {
    match name.split_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, name),
    }
}

/// Ask the database what a view's `SELECT *` expands to.
async fn view_columns(
    db: &AnyDatabase,
    schema: Option<&str>,
    name: &str,
) -> Result<Vec<String>, ApiError> {
    match db {
        AnyDatabase::Sqlite(db) => probe_view(db, schema, name).await,
        AnyDatabase::Postgres(db) => probe_view(db, schema, name).await,
        AnyDatabase::MySql(db) => probe_view(db, schema, name).await,
    }
}

async fn probe_view<Dr>(
    db: &Database<Dr>,
    schema: Option<&str>,
    name: &str,
) -> Result<Vec<String>, ApiError>
where
    Dr: Driver,
{
    let mut sql = Sql::<Dr::Dialect>::new();
    sql.push_raw("SELECT * FROM ");
    sql.push_table(schema, name);
    sql.push_raw(" LIMIT 1");

    let sql = sql.finish().map_err(ApiError::backend)?;
    let rows = db.driver().query(&sql).await.map_err(ApiError::backend)?;

    Ok(rows.first().and_then(Dr::column_names).unwrap_or_default())
}

/// Parse `column:asc,other:desc` against the target's columns.
fn parse_sort(raw: Option<&str>, target: &Target) -> Result<Vec<Sort>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (column, direction) = match part.split_once(':') {
                Some((column, direction)) => (column.trim(), direction.trim()),
                None => (part, "asc"),
            };

            let column = target
                .column(column)
                .ok_or_else(|| unknown_column(target, column))?;

            let direction = match direction.to_ascii_lowercase().as_str() {
                "asc" => Direction::Asc,
                "desc" => Direction::Desc,
                other => {
                    return Err(ApiError::bad_request(format!(
                        "`{other}` is not a sort direction; use `asc` or `desc`"
                    )));
                }
            };

            Ok(Sort {
                column: column.name.clone(),
                direction,
            })
        })
        .collect()
}

/// Decode the `filter` query parameter, which is JSON rather than a nested query syntax
/// because the values are strings of arbitrary content.
fn parse_filters(raw: Option<&str>) -> Result<Vec<Filter>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    serde_json::from_str(raw).map_err(|err| {
        ApiError::bad_request(format!("`filter` is not a JSON array of filters: {err}"))
    })
}

/// Check every filter names a real column and carries a value where one is needed.
///
/// Doing this before building SQL turns a typo into a 400 with the column's name in it,
/// rather than a database error from inside a subquery.
fn validate_filters(filters: &[Filter], target: &Target) -> Result<(), ApiError> {
    for filter in filters {
        if target.column(&filter.column).is_none() {
            return Err(unknown_column(target, &filter.column));
        }

        if filter.op.needs_value() && filter.value.is_none() {
            return Err(ApiError::bad_request(format!(
                "the `{}` filter on `{}` needs a value",
                filter.op.name(),
                filter.column
            )));
        }
    }

    Ok(())
}

fn unknown_column(target: &Target, name: &str) -> ApiError {
    ApiError::bad_request(format!(
        "`{name}` is not a column of `{}`",
        target.qualified_name()
    ))
}

/// The type a cell is cast to so that every backend returns text.
///
/// MySQL spells its variable-length text cast `CHAR`; SQLite and PostgreSQL both understand
/// `TEXT`.
fn cast_type<D: Dialect>() -> &'static str {
    if D::NAME == "mysql" { "CHAR" } else { "TEXT" }
}

/// Bind a value as text, casting it to the column's type where the dialect needs it.
///
/// PostgreSQL fixes parameter types when a statement is prepared, and a `TEXT` parameter
/// compared with an `INTEGER` column is a type error there. SQLite and MySQL coerce the
/// text for us, so the cast would only add parentheses.
fn bind_text<D: Dialect>(sql: &mut Sql<D>, ty: Option<&TypeDef>, value: &str) {
    if D::NAME == "postgres" {
        if let Some(ty) = ty {
            sql.push_raw("CAST(");
            sql.bind(value);
            sql.push_raw(" AS ");
            sql.push_raw(&D::type_name(ty.clone()));
            sql.push_raw(")");
            return;
        }
    }

    sql.bind(value);
}

/// Bind one JSON value for a column, casting on PostgreSQL for the same reason as
/// [`bind_text`].
fn bind_json<D: Dialect>(sql: &mut Sql<D>, ty: &TypeDef, value: &Value) {
    let text = json_text(value, ty);

    if D::NAME == "postgres" {
        sql.push_raw("CAST(");
        sql.bind(text);
        sql.push_raw(" AS ");
        sql.push_raw(&D::type_name(ty.clone()));
        sql.push_raw(")");
    } else {
        sql.bind(text);
    }
}

/// Turn a JSON value into the text a column is written from.
///
/// `null` is `None`, which binds SQL `NULL`. A JSON boolean becomes `1`/`0` for a boolean
/// column: PostgreSQL accepts both spellings for `BOOLEAN`, while SQLite and MySQL store
/// the column as an integer, and the words `true`/`false` would be stored as text there.
/// Everything else keeps its JSON spelling, which is the exact decimal for a number and the
/// JSON text for an object or an array.
fn json_text(value: &Value, ty: &TypeDef) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bool(value) => Some(match ty {
            TypeDef::Bool => if *value { "1" } else { "0" }.to_owned(),
            _ => value.to_string(),
        }),
        Value::String(value) => Some(value.clone()),
        other => Some(other.to_string()),
    }
}

/// Build the `WHERE` clause shared by the count and the page.
fn push_where<D: Dialect>(
    sql: &mut Sql<D>,
    target: &Target,
    filters: &[Filter],
) -> Result<(), ApiError> {
    if filters.is_empty() {
        return Ok(());
    }

    sql.push_raw(" WHERE ");
    for (index, filter) in filters.iter().enumerate() {
        if index > 0 {
            sql.push_raw(" AND ");
        }
        push_filter(sql, target, filter)?;
    }

    Ok(())
}

fn push_filter<D: Dialect>(
    sql: &mut Sql<D>,
    target: &Target,
    filter: &Filter,
) -> Result<(), ApiError> {
    let column = target
        .column(&filter.column)
        .ok_or_else(|| unknown_column(target, &filter.column))?;

    match filter.op {
        FilterOp::IsNull => {
            sql.push_ident(&column.name);
            sql.push_raw(" IS NULL");
            return Ok(());
        }
        FilterOp::IsNotNull => {
            sql.push_ident(&column.name);
            sql.push_raw(" IS NOT NULL");
            return Ok(());
        }
        FilterOp::Contains | FilterOp::StartsWith => {
            let value = require_value(filter)?;
            sql.push_raw("CAST(");
            sql.push_ident(&column.name);
            sql.push_raw(" AS ");
            sql.push_raw(cast_type::<D>());
            sql.push_raw(") LIKE ");
            bind_text::<D>(
                sql,
                None,
                &like_pattern(value, filter.op == FilterOp::Contains),
            );
            // `!` rather than a backslash, because MySQL treats a backslash inside a
            // string literal as an escape and the clause has to mean the same everywhere.
            sql.push_raw(" ESCAPE '!'");
            return Ok(());
        }
        op => {
            let value = require_value(filter)?;
            let operator = op.comparison().unwrap_or(" = ");
            sql.push_ident(&column.name);
            sql.push_raw(operator);
            bind_text::<D>(sql, Some(&column.ty), value);
        }
    }

    Ok(())
}

fn require_value(filter: &Filter) -> Result<&str, ApiError> {
    filter.value.as_deref().ok_or_else(|| {
        ApiError::bad_request(format!(
            "the `{}` filter on `{}` needs a value",
            filter.op.name(),
            filter.column
        ))
    })
}

/// Build a `LIKE` pattern, escaping the wildcards so a search for `100%` means those three
/// characters rather than "anything after 100".
fn like_pattern(value: &str, contains: bool) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    if contains {
        pattern.push('%');
    }
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '!') {
            pattern.push('!');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

/// `ORDER BY` the requested columns, then the primary key as a stable tie-break.
///
/// Without the tie-break, two rows with the same sort value can swap places between page
/// requests, and the UI shows a row twice while missing another.
fn push_order<D: Dialect>(sql: &mut Sql<D>, target: &Target, sorts: &[Sort]) {
    let mut wrote = false;

    for sort in sorts {
        if wrote {
            sql.push_raw(", ");
        } else {
            sql.push_raw(" ORDER BY ");
            wrote = true;
        }
        sql.push_ident(&sort.column);
        sql.push_raw(match sort.direction {
            Direction::Asc => " ASC",
            Direction::Desc => " DESC",
        });
    }

    for column in &target.primary_key {
        if wrote {
            sql.push_raw(", ");
        } else {
            sql.push_raw(" ORDER BY ");
            wrote = true;
        }
        sql.push_ident(column);
        sql.push_raw(" ASC");
    }
}

/// The number of rows the filters select.
async fn count_rows<Dr>(
    db: &Database<Dr>,
    target: &Target,
    filters: &[Filter],
) -> Result<u64, ApiError>
where
    Dr: Driver,
    String: Decode<Dr>,
{
    let mut sql = Sql::<Dr::Dialect>::new();
    sql.push_raw("SELECT CAST(COUNT(*) AS ");
    sql.push_raw(cast_type::<Dr::Dialect>());
    sql.push_raw(") FROM ");
    sql.push_table(target.schema.as_deref(), &target.name);
    push_where(&mut sql, target, filters)?;

    let sql = sql.finish().map_err(ApiError::backend)?.seal();
    let rows = db.driver().query(&sql).await.map_err(ApiError::backend)?;

    let Some(row) = rows.first() else {
        return Ok(0);
    };
    if Dr::is_null(row, 0) {
        return Ok(0);
    }

    let mut offset = 0;
    let text = String::decode(row, &mut offset).map_err(ApiError::backend)?;
    Ok(text.parse().unwrap_or(0))
}

/// Read one page, projecting every value into JSON.
///
/// Each backend decodes a cell as text, because the driver is the only layer that knows how
/// a row is laid out. `CAST` makes the server hand back text, `NULL` is projected through
/// [`Driver::is_null`], and the introspected column type turns a numeric or boolean cell
/// back into a JSON number or boolean.
async fn read_rows<Dr>(
    db: &Database<Dr>,
    target: &Target,
    sorts: &[Sort],
    filters: &[Filter],
    limit: u32,
    offset: u32,
) -> Result<Vec<Vec<Value>>, ApiError>
where
    Dr: Driver,
    String: Decode<Dr>,
{
    // A view can be empty, and an empty view has no row for the probe to name its columns
    // from. There is then no projection to build, and the honest answer is no rows.
    if target.columns.is_empty() {
        return Ok(Vec::new());
    }

    let mut sql = Sql::<Dr::Dialect>::new();
    sql.push_raw("SELECT ");

    for (index, column) in target.columns.iter().enumerate() {
        if index > 0 {
            sql.push_raw(", ");
        }
        sql.push_raw("CAST(");
        sql.push_ident(&column.name);
        sql.push_raw(" AS ");
        sql.push_raw(cast_type::<Dr::Dialect>());
        sql.push_raw(")");
    }

    sql.push_raw(" FROM ");
    sql.push_table(target.schema.as_deref(), &target.name);
    push_where(&mut sql, target, filters)?;
    push_order(&mut sql, target, sorts);
    sql.push_raw(" LIMIT ");
    sql.bind(i64::from(limit));
    sql.push_raw(" OFFSET ");
    sql.bind(i64::from(offset));

    let sql = sql.finish().map_err(ApiError::backend)?.seal();
    let rows = db.driver().query(&sql).await.map_err(ApiError::backend)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut values = Vec::with_capacity(target.columns.len());
        let mut decode_offset = 0;

        for column in &target.columns {
            if Dr::is_null(row, decode_offset) {
                values.push(Value::Null);
                decode_offset += 1;
            } else {
                let text = String::decode(row, &mut decode_offset).map_err(ApiError::backend)?;
                values.push(cell_value(&column.ty, text));
            }
        }

        out.push(values);
    }

    Ok(out)
}

/// Turn one text cell into JSON using the column's introspected type.
fn cell_value(ty: &TypeDef, text: String) -> Value {
    match ty {
        TypeDef::Bool => match text.as_str() {
            "1" | "t" | "true" | "TRUE" => Value::Bool(true),
            "0" | "f" | "false" | "FALSE" => Value::Bool(false),
            _ => Value::String(text),
        },
        TypeDef::Int2 | TypeDef::Int4 | TypeDef::Int8 | TypeDef::UInt(_) => integer_value(&text),
        TypeDef::Float4 | TypeDef::Float8 => float_value(text),
        TypeDef::Json | TypeDef::Jsonb => {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        }
        _ => Value::String(text),
    }
}

/// Parse an integer cell, keeping values outside `i64` as text rather than losing precision.
fn integer_value(text: &str) -> Value {
    if let Ok(value) = text.parse::<i64>() {
        return Value::from(value);
    }
    if let Ok(value) = text.parse::<u64>() {
        return Value::from(value);
    }
    Value::String(text.to_owned())
}

/// Parse a floating-point cell, falling back to text for `NaN` and infinities, which JSON
/// cannot represent.
fn float_value(text: String) -> Value {
    match text.parse::<f64>() {
        Ok(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::String(text)),
        Err(_) => Value::String(text),
    }
}

/// What `POST /api/tables/{table}/rows` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct InsertRequest {
    values: Map<String, Value>,
}

/// What `PATCH /api/tables/{table}/rows` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct UpdateRequest {
    key: Map<String, Value>,
    values: Map<String, Value>,
}

/// What `DELETE /api/tables/{table}/rows` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct DeleteRequest {
    key: Map<String, Value>,
}

/// Insert one row.
pub(crate) async fn insert_row(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Result<Json<InsertRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = json_body(body)?;
    let schema = state.db.introspect().await.map_err(ApiError::backend)?;
    let target = resolve_target(&name, &schema, &state.db).await?;

    let inserted = match &state.db {
        AnyDatabase::Sqlite(db) => insert(db, &state.db, &target, &request.values).await,
        AnyDatabase::Postgres(db) => insert(db, &state.db, &target, &request.values).await,
        AnyDatabase::MySql(db) => insert(db, &state.db, &target, &request.values).await,
    }?;

    Ok(Json(json!({ "inserted": inserted })))
}

/// Update one row, identified by its primary key.
pub(crate) async fn update_row(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Result<Json<UpdateRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = json_body(body)?;
    let schema = state.db.introspect().await.map_err(ApiError::backend)?;
    let target = resolve_target(&name, &schema, &state.db).await?;

    require_primary_key(&target)?;
    require_key(&target, &request.key)?;

    let updated = match &state.db {
        AnyDatabase::Sqlite(db) => {
            update(db, &state.db, &target, &request.key, &request.values).await
        }
        AnyDatabase::Postgres(db) => {
            update(db, &state.db, &target, &request.key, &request.values).await
        }
        AnyDatabase::MySql(db) => {
            update(db, &state.db, &target, &request.key, &request.values).await
        }
    }?;

    Ok(Json(json!({ "updated": updated })))
}

/// Delete one row, identified by its primary key.
pub(crate) async fn delete_row(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Result<Json<DeleteRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = json_body(body)?;
    let schema = state.db.introspect().await.map_err(ApiError::backend)?;
    let target = resolve_target(&name, &schema, &state.db).await?;

    require_primary_key(&target)?;
    require_key(&target, &request.key)?;

    let deleted = match &state.db {
        AnyDatabase::Sqlite(db) => delete(db, &state.db, &target, &request.key).await,
        AnyDatabase::Postgres(db) => delete(db, &state.db, &target, &request.key).await,
        AnyDatabase::MySql(db) => delete(db, &state.db, &target, &request.key).await,
    }?;

    Ok(Json(json!({ "deleted": deleted })))
}

fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|rejection| ApiError::bad_request(rejection.to_string()))
}

/// A write can only name a row when the table says which columns identify one.
fn require_primary_key(target: &Target) -> Result<(), ApiError> {
    if target.primary_key.is_empty() {
        return Err(ApiError::bad_request(format!(
            "`{}` has no primary key, so the studio cannot tell which row to change; \
             use the SQL console instead",
            target.qualified_name()
        )));
    }

    Ok(())
}

/// The key must name every primary-key column; extra columns would make the `WHERE`
/// narrower, which is still unambiguous, so they are permitted.
fn require_key(target: &Target, key: &Map<String, Value>) -> Result<(), ApiError> {
    if key.is_empty() {
        return Err(ApiError::bad_request(
            "the request needs a `key` naming the row",
        ));
    }

    for column in &target.primary_key {
        if !key.contains_key(column) {
            return Err(ApiError::bad_request(format!(
                "the key is missing primary-key column `{column}`; `{}` needs all of {}",
                target.qualified_name(),
                target.primary_key.join(", ")
            )));
        }
    }

    for name in key.keys() {
        if target.column(name).is_none() {
            return Err(unknown_column(target, name));
        }
    }

    Ok(())
}

/// Insert one row, returning the number of rows the server says it wrote.
async fn insert<Dr>(
    _db: &Database<Dr>,
    any: &AnyDatabase,
    target: &Target,
    values: &Map<String, Value>,
) -> Result<u64, ApiError>
where
    Dr: Driver,
    <<Dr as Driver>::Dialect as Dialect>::Value: Into<SqlValue>,
{
    if values.is_empty() {
        return Err(ApiError::bad_request(
            "an insert needs at least one column value",
        ));
    }

    let mut sql = Sql::<Dr::Dialect>::with_capacity(64 + values.len() * 24);
    sql.push_raw("INSERT INTO ");
    sql.push_table(target.schema.as_deref(), &target.name);
    sql.push_raw(" (");

    for (index, name) in values.keys().enumerate() {
        if index > 0 {
            sql.push_raw(", ");
        }
        sql.push_ident(name);
    }

    sql.push_raw(") VALUES (");

    for (index, (name, value)) in values.iter().enumerate() {
        if index > 0 {
            sql.push_raw(", ");
        }
        let column = target
            .column(name)
            .ok_or_else(|| unknown_column(target, name))?;
        bind_json(&mut sql, &column.ty, value);
    }

    sql.push_raw(")");

    execute(any, sql.finish().map_err(ApiError::backend)?.seal()).await
}

/// Update one row, returning the number of rows the server says it wrote.
async fn update<Dr>(
    _db: &Database<Dr>,
    any: &AnyDatabase,
    target: &Target,
    key: &Map<String, Value>,
    values: &Map<String, Value>,
) -> Result<u64, ApiError>
where
    Dr: Driver,
    <<Dr as Driver>::Dialect as Dialect>::Value: Into<SqlValue>,
{
    if values.is_empty() {
        return Err(ApiError::bad_request(
            "an update needs at least one column value",
        ));
    }

    let mut sql = Sql::<Dr::Dialect>::with_capacity(96 + values.len() * 24 + key.len() * 24);
    sql.push_raw("UPDATE ");
    sql.push_table(target.schema.as_deref(), &target.name);
    sql.push_raw(" SET ");

    for (index, (name, value)) in values.iter().enumerate() {
        if index > 0 {
            sql.push_raw(", ");
        }
        let column = target
            .column(name)
            .ok_or_else(|| unknown_column(target, name))?;
        sql.push_ident(name);
        sql.push_raw(" = ");
        bind_json(&mut sql, &column.ty, value);
    }

    sql.push_raw(" WHERE ");
    push_key(&mut sql, target, key)?;

    execute(any, sql.finish().map_err(ApiError::backend)?.seal()).await
}

/// Delete one row, returning the number of rows the server says it removed.
async fn delete<Dr>(
    _db: &Database<Dr>,
    any: &AnyDatabase,
    target: &Target,
    key: &Map<String, Value>,
) -> Result<u64, ApiError>
where
    Dr: Driver,
    <<Dr as Driver>::Dialect as Dialect>::Value: Into<SqlValue>,
{
    let mut sql = Sql::<Dr::Dialect>::with_capacity(64 + key.len() * 24);
    sql.push_raw("DELETE FROM ");
    sql.push_table(target.schema.as_deref(), &target.name);
    sql.push_raw(" WHERE ");
    push_key(&mut sql, target, key)?;

    execute(any, sql.finish().map_err(ApiError::backend)?.seal()).await
}

/// Bind every key column as a `WHERE` equality.
fn push_key<D: Dialect>(
    sql: &mut Sql<D>,
    target: &Target,
    key: &Map<String, Value>,
) -> Result<(), ApiError> {
    for (index, (name, value)) in key.iter().enumerate() {
        if index > 0 {
            sql.push_raw(" AND ");
        }

        let column = target
            .column(name)
            .ok_or_else(|| unknown_column(target, name))?;

        if value.is_null() {
            return Err(ApiError::bad_request(format!(
                "the key value for `{name}` is null; a primary key cannot be null"
            )));
        }

        sql.push_ident(name);
        sql.push_raw(" = ");
        bind_json(sql, &column.ty, value);
    }

    Ok(())
}

/// Run a finished statement through whichever backend the router owns.
///
/// The statement's dialect is the one that built it; [`Fragment`] keeps that association and
/// refuses to re-render it for another backend, so a mixed-up variant is a loud error rather
/// than a statement sent to the wrong server.
async fn execute<D: Dialect>(any: &AnyDatabase, sql: Sql<D>) -> Result<u64, ApiError>
where
    D::Value: Into<SqlValue>,
{
    let output = any
        .execute(Fragment::from(sql))
        .await
        .map_err(ApiError::backend)?;
    Ok(output.rows_affected)
}

/// What `POST /api/query` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct QueryRequest {
    /// One SQL statement.
    sql: String,
}

/// What one SQL statement did.
struct Outcome {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    rows_affected: u64,
    last_insert_id: Option<u64>,
}

/// Run one SQL statement and report what it produced.
///
/// The request is deliberately narrow: the statement has to be exactly one according to the
/// project's own splitter, so a pasted script is refused rather than half-run.
pub(crate) async fn query(
    State(state): State<AppState>,
    body: Result<Json<QueryRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = json_body(body)?;
    let started = Instant::now();
    let statement = one_statement(&request.sql)?;

    let outcome = match &state.db {
        AnyDatabase::Sqlite(db) => run_statement(db, &state.db, &statement).await,
        AnyDatabase::Postgres(db) => run_statement(db, &state.db, &statement).await,
        AnyDatabase::MySql(db) => run_statement(db, &state.db, &statement).await,
    }?;

    Ok(Json(json!({
        "columns": outcome.columns,
        "rows": outcome.rows,
        "rows_affected": outcome.rows_affected,
        "last_insert_id": outcome.last_insert_id,
        "duration_ms": elapsed_ms(started),
    })))
}

/// Take the one statement out of the request, or explain why there is not exactly one.
fn one_statement(sql: &str) -> Result<String, ApiError> {
    let mut statements = split_statements(sql);

    match statements.len() {
        0 => Err(ApiError::bad_request(
            "there is no SQL statement in the request",
        )),
        1 => Ok(statements.remove(0)),
        count => Err(ApiError::bad_request(format!(
            "the studio runs exactly one statement at a time, and the request contains \
             {count}; run them one by one"
        ))),
    }
}

/// Run one statement, preferring a `SELECT` and falling back to a plain execution.
///
/// A statement is a `SELECT` if wrapping it in a derived table works. That probe is a second
/// execution of a read, which costs one round trip and is safe. A statement that is not a
/// `SELECT` is executed exactly once, so an `INSERT` is never run twice.
async fn run_statement<Dr>(
    db: &Database<Dr>,
    any: &AnyDatabase,
    statement: &str,
) -> Result<Outcome, ApiError>
where
    Dr: Driver,
    String: Decode<Dr>,
{
    match probe(db, statement).await.map_err(ApiError::backend)? {
        Probe::Failed => {
            let output = any
                .execute(Fragment::new(statement.to_owned()))
                .await
                .map_err(ApiError::backend)?;

            Ok(Outcome {
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: output.rows_affected,
                last_insert_id: output.last_insert_id,
            })
        }
        // The probe worked but there is nothing to read names from. Guessing would put a
        // header the user cannot trust above an empty grid, so there is no header.
        Probe::Empty => Ok(Outcome {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: 0,
            last_insert_id: None,
        }),
        Probe::Columns(columns) => {
            let rows = select_as_text(db, statement, &columns).await?;

            Ok(Outcome {
                columns,
                rows,
                rows_affected: 0,
                last_insert_id: None,
            })
        }
    }
}

/// What wrapping a statement in `SELECT * FROM (...) AS __besu_q LIMIT 1` found.
enum Probe {
    /// The statement is not a query; it has to be executed instead.
    Failed,
    /// It is a query, but it returned no row to take column names from.
    Empty,
    /// It is a query with these columns.
    Columns(Vec<String>),
}

/// Ask the database whether a statement is a query, and what it returns.
async fn probe<Dr>(db: &Database<Dr>, statement: &str) -> Result<Probe, Dr::Error>
where
    Dr: Driver,
{
    let mut sql = Sql::<Dr::Dialect>::new();
    sql.push_raw("SELECT * FROM (");
    sql.push_raw(statement);
    sql.push_raw(") AS __besu_q LIMIT 1");

    let sql = sql.finish().map_err(Dr::error)?;
    let rows = match db.driver().query(&sql).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::debug!(error = %err, "the statement is not a query");
            return Ok(Probe::Failed);
        }
    };

    Ok(match rows.first().and_then(Dr::column_names) {
        Some(columns) if !columns.is_empty() => Probe::Columns(columns),
        _ => Probe::Empty,
    })
}

/// Re-read a query with every cell cast to text.
///
/// The original column names are known from the probe and are kept for the response; the
/// casts need aliases the driver can decode in a known order, so they are numbered.
async fn select_as_text<Dr>(
    db: &Database<Dr>,
    statement: &str,
    columns: &[String],
) -> Result<Vec<Vec<Value>>, ApiError>
where
    Dr: Driver,
    String: Decode<Dr>,
{
    let mut sql = Sql::<Dr::Dialect>::with_capacity(statement.len() + columns.len() * 32 + 32);
    sql.push_raw("SELECT ");

    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            sql.push_raw(", ");
        }
        sql.push_raw("CAST(q.");
        sql.push_ident(column);
        sql.push_raw(" AS ");
        sql.push_raw(cast_type::<Dr::Dialect>());
        sql.push_raw(") AS ");
        sql.push_ident(&format!("__c{index}"));
    }

    sql.push_raw(" FROM (");
    sql.push_raw(statement);
    sql.push_raw(") AS q LIMIT ");
    sql.bind(i64::from(QUERY_LIMIT));

    let sql = sql.finish().map_err(ApiError::backend)?.seal();
    let rows = db.driver().query(&sql).await.map_err(ApiError::backend)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut values = Vec::with_capacity(columns.len());
        let mut offset = 0;

        for _ in columns {
            if Dr::is_null(row, offset) {
                values.push(Value::Null);
                offset += 1;
            } else {
                let text = String::decode(row, &mut offset).map_err(ApiError::backend)?;
                values.push(Value::String(text));
            }
        }

        out.push(values);
    }

    Ok(out)
}

/// A duration a JSON number can hold.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A failure the API can report as JSON.
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn backend(err: impl core::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::database_label;

    #[test]
    fn a_database_label_is_the_last_path_segment() {
        // Both SQLite spellings, the common server URL, and an option that contains no
        // slash but must still not be mistaken for the database name.
        assert_eq!(database_label("sqlite:///tmp/app.db"), "app.db");
        assert_eq!(database_label("sqlite://app.db"), "app.db");
        assert_eq!(database_label("postgres://besu@localhost:5432/app"), "app");
        assert_eq!(database_label("mysql://root@127.0.0.1/db?ssl=true"), "db");

        // A DSN has no path to read, so it is shown whole.
        assert_eq!(
            database_label("host=localhost user=besu dbname=app"),
            "host=localhost user=besu dbname=app"
        );
    }
}
