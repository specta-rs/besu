//! Getting a [`Request`] to the server and a [`Response`] back.

use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{BridgeError, Request, Response};

/// Carries a request to the server and brings the answer back.
///
/// A transport only moves JSON-shaped data; it does not build SQL, bind parameters or decode
/// rows. That is why [`Bridge`](crate::Bridge) can be generic over one: the browser's `fetch`
/// and a test's closure differ in exactly one method.
///
/// The future must be `Send` because [`Driver`](besu_core::Driver) says so, even though a
/// wasm module is single-threaded. The browser implementation satisfies that by moving the
/// `!Send` JavaScript work onto `spawn_local` and awaiting a `Send` channel; see
/// `FetchTransport` (on `wasm32` targets).
pub trait Transport: Clone + Send + Sync + 'static {
    /// Post `request` and wait for the response.
    ///
    /// A transport-level failure — a network error, a non-JSON body, a cancelled fetch — is
    /// [`BridgeError::Transport`] or [`BridgeError::Protocol`]. A statement the server ran
    /// and rejected arrives as a successful `Response` with [`Response::error`] set, so the
    /// caller's [`BridgeError::Server`] says what the database said.
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, BridgeError>> + Send;
}

/// The closure a [`MemoryTransport`] answers with.
type Handler = dyn Fn(&Request) -> Result<Response, BridgeError> + Send + Sync;

/// A transport that answers from a closure, for tests and for a local server.
///
/// It records every request it sees, so a test can assert on the SQL and the encoded
/// parameters as well as on the decoded rows.
///
/// ```
/// use besu_bridge::{BridgeValue, MemoryTransport, Request, Response};
///
/// let transport = MemoryTransport::new(|request: &Request| {
///     assert_eq!(request.sql, "SELECT 1");
///     Ok(Response::rows(
///         Some(vec!["one".to_owned()]),
///         vec![vec![BridgeValue::Int(1)]],
///     ))
/// });
/// assert_eq!(transport.requests().len(), 0);
/// ```
#[derive(Clone)]
pub struct MemoryTransport {
    handler: Arc<Handler>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl MemoryTransport {
    /// Build a transport backed by `handler`.
    ///
    /// The handler runs synchronously inside [`Transport::send`]; returning an error is how a
    /// test simulates a broken connection, and returning `Ok(Response::error(..))` is how it
    /// simulates a database error.
    pub fn new(
        handler: impl Fn(&Request) -> Result<Response, BridgeError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Arc::new(handler),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every request this transport has answered, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.lock().clone()
    }

    /// The most recent request, if there has been one.
    pub fn last_request(&self) -> Option<Request> {
        self.lock().last().cloned()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Request>> {
        // A handler that panics poisons the mutex; the recorded requests are still valid, so
        // recover them rather than turning a test failure into a second, unrelated one.
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Transport for MemoryTransport {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, BridgeError>> + Send {
        let this = self.clone();
        async move {
            this.lock().push(request.clone());
            (this.handler)(&request)
        }
    }
}
