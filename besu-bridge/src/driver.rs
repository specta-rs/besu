//! The [`Driver`] implementation: build a request, send it, decode what comes back.

use std::{marker::PhantomData, sync::Arc};

use besu_core::{Dialect, Driver, Sql};

use crate::{BridgeDialect, BridgeError, BridgeValue, Request, Transport};

/// A row returned by the bridge, with the column names the server reported.
///
/// The values are already parsed from JSON and keep their wire type; a [`Decode`] impl reads
/// one of them and converts it, without parsing anything again.
///
/// [`Decode`]: besu_core::Decode
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeRow {
    columns: Arc<[String]>,
    values: Vec<BridgeValue>,
}

impl BridgeRow {
    /// Build a row from column names and values in column order.
    pub fn new(columns: impl Into<Arc<[String]>>, values: Vec<BridgeValue>) -> Self {
        Self {
            columns: columns.into(),
            values,
        }
    }

    /// The column names the server reported, which may be empty.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// The values, in column order.
    pub fn values(&self) -> &[BridgeValue] {
        &self.values
    }

    /// The value at `index`, or `None` when the row is shorter than that.
    pub fn value(&self, index: usize) -> Option<&BridgeValue> {
        self.values.get(index)
    }
}

/// A Besu driver that runs statements on a server through a [`Transport`].
///
/// `D` is the dialect the server's database speaks, and it decides how the browser spells the
/// SQL: `Bridge<_, Postgres>` writes `$1` and reads `{"t":"int","v":1}` back as an `i64`,
/// while `Bridge<_, MySql>` writes `?`. The bridge itself never connects to a database.
///
/// ```ignore
/// use besu::prelude::*;
/// use besu_bridge::{Bridge, FetchTransport};
/// use besu_postgres::Postgres;
///
/// let db = Database::new(Bridge::<_, Postgres>::new(FetchTransport::new("/api/query")));
/// let adults: Vec<User> = db.select(users).filter(users.age.gt(18)).await?;
/// ```
pub struct Bridge<T: Transport, D: Dialect> {
    transport: T,
    dialect: PhantomData<fn() -> D>,
}

impl<T: Transport, D: Dialect> std::fmt::Debug for Bridge<T, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("transport", &std::any::type_name::<T>())
            .field("dialect", &D::NAME)
            .finish()
    }
}

impl<T: Transport, D: Dialect> Bridge<T, D> {
    /// Wrap a transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            dialect: PhantomData,
        }
    }

    /// The transport this bridge sends through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Consume the bridge and return its transport, for a caller that wants to keep it.
    pub fn into_transport(self) -> T {
        self.transport
    }

    fn request(&self, kind: crate::RequestKind, sql: &Sql<BridgeDialect<D>>) -> Request {
        Request {
            sql: sql.sql().to_owned(),
            params: sql.args().to_vec(),
            kind,
        }
    }
}

impl<T: Transport, D: Dialect> Clone for Bridge<T, D> {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
            dialect: PhantomData,
        }
    }
}

impl<T: Transport, D: Dialect> Driver for Bridge<T, D> {
    type Dialect = BridgeDialect<D>;
    type Error = BridgeError;
    type Row = BridgeRow;
    type Output = u64;

    fn error(err: besu_core::Error) -> Self::Error {
        err.into()
    }

    fn row_len(row: &Self::Row) -> usize {
        row.values.len()
    }

    fn is_null(row: &Self::Row, index: usize) -> bool {
        // Out of range is `true`, as the trait requires: an outer join that did not match
        // produces a short row, and that row is absent rather than an error.
        row.values.get(index).is_none_or(BridgeValue::is_null)
    }

    fn column_names(row: &Self::Row) -> Option<Vec<String>> {
        // A server that cannot name its columns sends none; that is the trait's `None`, not
        // an empty list, so callers can tell "nothing to display" from "zero columns".
        if row.columns.is_empty() {
            None
        } else {
            Some(row.columns.to_vec())
        }
    }

    fn decode_text(row: &Self::Row, index: usize) -> Result<String, Self::Error> {
        match row.values.get(index) {
            Some(
                BridgeValue::Text(text) | BridgeValue::EnumLabel(text) | BridgeValue::Decimal(text),
            ) => Ok(text.clone()),
            Some(BridgeValue::Json(value)) => Ok(value.to_string()),
            Some(other) => Err(Self::error(besu_core::Error::decode(
                index,
                format!("cannot read a `{}` value as text", other.tag()),
            ))),
            None => Err(Self::error(besu_core::Error::decode(
                index,
                "the row has no value at this column",
            ))),
        }
    }

    async fn query(&self, sql: &Sql<Self::Dialect>) -> Result<Vec<Self::Row>, Self::Error> {
        let response = self
            .transport
            .send(self.request(crate::RequestKind::Query, sql))
            .await?;
        let (columns, rows) = response.into_rows()?;
        let columns: Arc<[String]> = columns.into();
        Ok(rows
            .into_iter()
            .map(|values| BridgeRow {
                columns: columns.clone(),
                values,
            })
            .collect())
    }

    async fn execute(&self, sql: &Sql<Self::Dialect>) -> Result<Self::Output, Self::Error> {
        let response = self
            .transport
            .send(self.request(crate::RequestKind::Execute, sql))
            .await?;
        response.into_affected()
    }
}
