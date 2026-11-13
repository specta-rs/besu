//! The server half: run a [`Request`] against a real driver and serialise the answer.
//!
//! The client half is complete on its own; this module exists so the endpoint it talks to is
//! specified as code rather than prose. A server owns a driver and knows how to read that
//! driver's rows, which is exactly the part [`besu_core::Driver`] deliberately leaves opaque.

use besu_core::Driver;

use crate::{BridgeError, BridgeValue, Request, Response};

/// Turns a driver's opaque rows and output into the bridge's wire values.
///
/// Implement this next to the driver you serve. `D::Row` and `D::Output` belong to the
/// driver, so only the code that chose that driver can read them; the trait is the seam where
/// that knowledge meets the HTTP handler.
///
/// ```ignore
/// use besu_bridge::{BridgeError, BridgeSource, BridgeValue};
/// use besu_postgres::Sqlx;
///
/// struct PostgresRows;
///
/// impl BridgeSource<Sqlx> for PostgresRows {
///     fn encode_row(&self, row: &sqlx::postgres::PgRow) -> Result<Vec<BridgeValue>, BridgeError> {
///         // The server knows the statement's column types, so it reads each column with
///         // `row.try_get(index)` and converts it with `BridgeValue::from(Value::from(..))`.
///         ...
///     }
///
///     fn affected(&self, output: &sqlx::postgres::PgQueryResult) -> u64 {
///         output.rows_affected()
///     }
/// }
/// ```
pub trait BridgeSource<D: Driver> {
    /// Encode one row, in column order.
    ///
    /// The column names are read separately with [`Driver::column_names`], so this only has
    /// to produce the values. A failure becomes an error response; it is never replaced with
    /// an empty row.
    fn encode_row(&self, row: &D::Row) -> Result<Vec<BridgeValue>, BridgeError>;

    /// Read the affected-row count out of the driver's output.
    ///
    /// [`Driver::Output`] is opaque, so this is the server's decision: SQLite's
    /// `SqliteQueryResult` and PostgreSQL's `PgQueryResult` both have a count, while a driver
    /// that only reports success can return zero.
    fn affected(&self, output: &D::Output) -> u64;
}

/// Run `request` against `driver` and return the response to send back.
///
/// A query's column names come from its first row, because that is the only place
/// [`Driver`] exposes them; a result set with no rows therefore has no names. Every other
/// failure — a malformed parameter, a driver error, a row the source cannot encode — becomes
/// [`Response::error`], so the handler can always send a 200 with a well-formed body and let
/// the client raise [`BridgeError::Server`].
///
/// ```ignore
/// use besu_bridge::{BridgeSource, Request, Response, handle_request};
///
/// async fn endpoint(sqlx: &besu_postgres::Sqlx, body: String) -> Response {
///     match serde_json::from_str::<Request>(&body) {
///         Ok(request) => handle_request(sqlx, &PostgresRows, request).await,
///         Err(err) => Response::error(format!("not a bridge request: {err}")),
///     }
/// }
/// ```
pub async fn handle_request<D, S>(driver: &D, source: &S, request: Request) -> Response
where
    D: Driver,
    S: BridgeSource<D> + Sync,
{
    let sql = match request.to_sql::<D::Dialect>() {
        Ok(sql) => sql,
        Err(err) => return Response::error(err.to_string()),
    };

    match request.kind {
        crate::RequestKind::Query => match driver.query(&sql).await {
            Ok(rows) => {
                let columns = rows.first().and_then(D::column_names);
                let mut encoded = Vec::with_capacity(rows.len());
                for row in &rows {
                    match source.encode_row(row) {
                        Ok(values) => encoded.push(values),
                        Err(err) => return Response::error(err.to_string()),
                    }
                }
                Response::rows(columns, encoded)
            }
            Err(err) => Response::error(err.to_string()),
        },
        crate::RequestKind::Execute => match driver.execute(&sql).await {
            Ok(output) => Response::affected(source.affected(&output)),
            Err(err) => Response::error(err.to_string()),
        },
    }
}
