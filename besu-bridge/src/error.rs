use crate::ServerError;

/// Everything that can go wrong while running a statement through the bridge.
///
/// A query has one `?` type, as with every driver: a value the dialect cannot bind, a
/// transport failure, a server error and a malformed response all arrive here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BridgeError {
    /// The request never reached the server, or its reply never arrived.
    #[error("the bridge transport failed: {0}")]
    Transport(String),

    /// The server ran the statement and reported an error.
    #[error("the bridge server failed: {0}")]
    Server(#[from] ServerError),

    /// The statement could not be built or a value could not be encoded, decoded or
    /// represented by the dialect.
    #[error(transparent)]
    Query(#[from] besu_core::Error),

    /// The server answered with JSON that is not a valid bridge exchange.
    #[error("the bridge response is malformed: {0}")]
    Protocol(String),

    /// The exchange could not be serialised as JSON.
    #[error("the bridge could not serialise the exchange: {0}")]
    Json(#[from] serde_json::Error),
}

impl BridgeError {
    /// Build a [`BridgeError::Query`] for the column at `index`.
    ///
    /// Decoders report the column they were reading, which is the only location a row
    /// decoder knows.
    pub fn decode(index: usize, reason: impl Into<String>) -> Self {
        Self::Query(besu_core::Error::decode(index, reason))
    }
}
