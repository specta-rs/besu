use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use besu::{
    Decode, Dialect, Driver, Fragment, Sql, TypeDef,
    schema::{Table, migrate::split_statements},
};
use besu_any::AnyDatabase;
use serde::Deserialize;
use serde_json::{Value, json};

/// The page size when a request does not ask for one.
const DEFAULT_LIMIT: u32 = 50;

/// The most rows a single request may ask for.
const MAX_LIMIT: u32 = 500;

/// The warning every query response carries.
const ARBITRARY_SQL_WARNING: &str = "besu-studio runs arbitrary SQL against the configured \
     database. It has no authentication and no read-only mode, so use it only against a \
     development database.";

/// The one page the studio serves.
pub(crate) async fn index() -> Html<&'static str> {
    Html(include_str!("assets/index.html"))
}

/// The introspected schema, serialised as-is.
pub(crate) async fn schema(
    State(db): State<AnyDatabase>,
) -> Result<Json<besu::schema::Schema>, ApiError> {
    db.introspect().await.map(Json).map_err(ApiError::backend)
}

/// What `GET /api/tables/{table}/rows` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct RowQuery {
    /// How many rows to return, capped at [`MAX_LIMIT`].
    limit: Option<u32>,
    /// How many rows to skip.
    offset: Option<u32>,
}

/// A page of rows from one table.
///
/// The table name is checked against the introspected schema before any SQL is built. The
/// name never reaches the statement verbatim: the statement is built through [`Sql`], which
/// quotes every identifier the dialect's own way.
pub(crate) async fn table_rows(
    State(db): State<AnyDatabase>,
    Path(name): Path<String>,
    Query(page): Query<RowQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = page.offset.unwrap_or(0);

    let schema = db.introspect().await.map_err(ApiError::backend)?;
    let table = schema
        .get(&name)
        .or_else(|| schema.table(&name))
        .ok_or_else(|| ApiError::not_found(format!("no table named `{name}` in the database")))?;

    let rows = match &db {
        AnyDatabase::Sqlite(db) => read_rows(db, table, limit, offset).await,
        AnyDatabase::Postgres(db) => read_rows(db, table, limit, offset).await,
        AnyDatabase::MySql(db) => read_rows(db, table, limit, offset).await,
    }
    .map_err(ApiError::backend)?;

    let columns: Vec<&str> = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();

    Ok(Json(json!({
        "table": table.qualified_name(),
        "columns": columns,
        "rows": rows,
        "limit": limit,
        "offset": offset,
    })))
}

/// What `POST /api/query` accepts.
#[derive(Debug, Deserialize)]
pub(crate) struct QueryRequest {
    /// One SQL statement.
    sql: String,
}

/// Run one SQL statement and report what it changed.
///
/// The request is deliberately narrow: the statement has to be exactly one according to the
/// project's own splitter, so a pasted script is refused rather than half-run. Everything the
/// database accepts as a single statement is allowed, including `INSERT`, `UPDATE` and
/// `DELETE`; this is a development tool, not a read-only console.
pub(crate) async fn query(
    State(db): State<AnyDatabase>,
    Json(request): Json<QueryRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut statements = split_statements(&request.sql);

    if statements.is_empty() {
        return Err(ApiError::bad_request(
            "there is no SQL statement in the request",
        ));
    }

    if statements.len() > 1 {
        return Err(ApiError::bad_request(format!(
            "the studio runs exactly one statement at a time, and the request contains {}; \
             run them one by one",
            statements.len()
        )));
    }

    let statement = statements.remove(0);
    let output = db
        .execute(Fragment::new(statement))
        .await
        .map_err(ApiError::backend)?;

    Ok(Json(json!({
        "rows_affected": output.rows_affected,
        "last_insert_id": output.last_insert_id,
        "warning": ARBITRARY_SQL_WARNING,
    })))
}

/// Read one page of a table, projecting every value into JSON.
///
/// Each backend's own [`Decode`] implementation reads a cell as text, because the driver is
/// the only layer that knows how a row is laid out. `CAST` is applied in the statement so
/// every backend hands back text, `NULL` is projected through [`Driver::is_null`], and the
/// introspected column type turns a numeric or boolean cell back into a JSON number or
/// boolean. A type Besu does not model stays a string, which is the honest representation.
async fn read_rows<Dr>(
    db: &besu::Database<Dr>,
    table: &Table,
    limit: u32,
    offset: u32,
) -> Result<Vec<Vec<Value>>, Dr::Error>
where
    Dr: Driver,
    String: Decode<Dr>,
{
    let mut sql = Sql::<Dr::Dialect>::new();
    sql.push_raw("SELECT ");

    for (index, column) in table.columns.iter().enumerate() {
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
    sql.push_table(table.schema.as_deref(), &table.name);
    sql.push_raw(" LIMIT ");
    sql.bind(i64::from(limit));
    sql.push_raw(" OFFSET ");
    sql.bind(i64::from(offset));

    let sql = sql.finish().map_err(Dr::error)?;
    let rows = db.driver().query(&sql).await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut values = Vec::with_capacity(table.columns.len());
        let mut decode_offset = 0;

        for (index, column) in table.columns.iter().enumerate() {
            if Dr::is_null(row, index) {
                values.push(Value::Null);
                decode_offset += 1;
            } else {
                let text = String::decode(row, &mut decode_offset)?;
                values.push(cell_value(&column.ty, text));
            }
        }

        out.push(values);
    }

    Ok(out)
}

/// The type a cell is cast to so that every backend returns text.
///
/// MySQL spells its variable-length text cast `CHAR`; SQLite and PostgreSQL both understand
/// `TEXT`.
fn cast_type<D: Dialect>() -> &'static str {
    if D::NAME == "mysql" { "CHAR" } else { "TEXT" }
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
