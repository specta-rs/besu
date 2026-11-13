//! The browser transport: `fetch` a JSON body to an endpoint.

use std::future::Future;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::{BridgeError, Request, Response, Transport};

/// Posts requests to a URL with the browser's `fetch`.
///
/// ```ignore
/// use besu_bridge::{Bridge, FetchTransport};
/// use besu_postgres::Postgres;
///
/// let bridge = Bridge::<_, Postgres>::new(
///     FetchTransport::new("/api/query").with_header("authorization", token),
/// );
/// ```
///
/// The server should answer with the JSON of a [`Response`] — `{"rows": [...]}` or
/// `{"affected": n}`, and `{"error": {"message": "..."}}` when the statement fails. A non-2xx
/// status is treated as a transport failure unless the body is itself a bridge response, so a
/// server can report a database error with a 400 and still be understood.
#[derive(Debug, Clone)]
pub struct FetchTransport {
    url: String,
    headers: Vec<(String, String)>,
}

impl FetchTransport {
    /// A transport that POSTs to `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// Add a request header, such as an authorization token.
    ///
    /// Called repeatedly, later values replace earlier ones through `Headers::set`.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The endpoint this transport posts to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Perform the fetch.
    ///
    /// This future borrows JavaScript handles, so it is `!Send`. [`Transport::send`] calls it
    /// from a `spawn_local` task and relays the result through a channel.
    async fn post(&self, request: Request) -> Result<Response, BridgeError> {
        let body = serde_json::to_string(&request)?;

        let headers = web_sys::Headers::new().map_err(js_error)?;
        headers
            .set("content-type", "application/json")
            .map_err(js_error)?;
        for (name, value) in &self.headers {
            headers.set(name, value).map_err(js_error)?;
        }

        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_mode(web_sys::RequestMode::Cors);
        init.set_headers(&headers);
        init.set_body(&JsValue::from_str(&body));

        let request =
            web_sys::Request::new_with_str_and_init(&self.url, &init).map_err(js_error)?;
        let window = web_sys::window().ok_or_else(|| {
            BridgeError::Transport("there is no window to fetch from in this context".to_owned())
        })?;

        let promise = window.fetch_with_request(&request);
        let response: web_sys::Response = JsFuture::from(promise)
            .await
            .map_err(js_error)?
            .dyn_into()
            .map_err(js_error)?;
        let status = response.status();
        let ok = response.ok();

        let text = JsFuture::from(response.text().map_err(js_error)?)
            .await
            .map_err(js_error)?;
        let text = text.as_string().ok_or_else(|| {
            BridgeError::Protocol("the bridge response body was not text".to_owned())
        })?;

        match serde_json::from_str::<Response>(&text) {
            Ok(response) => Ok(response),
            // A proxy or a framework in front of the bridge may answer with HTML or plain
            // text. Its status is the useful part, so report both.
            Err(_) if !ok => Err(BridgeError::Transport(format!("HTTP {status}: {text}"))),
            Err(err) => Err(BridgeError::Protocol(format!(
                "HTTP {status} returned a body that is not a bridge response: {err}; body: {text}"
            ))),
        }
    }
}

impl Transport for FetchTransport {
    /// Start the fetch on the local task queue and await its result through a channel.
    ///
    /// `Driver::query` and `Driver::execute` return `impl Future + Send`, because the trait
    /// is written for a thread pool. A fetch future holds `web_sys` handles, which are
    /// JavaScript values and therefore neither `Send` nor `Sync`, so awaiting it directly
    /// would make the driver's future `!Send`. `spawn_local` has no such bound, and
    /// `futures_channel::oneshot` is `Send` when its payload is, which [`Response`] and
    /// [`BridgeError`] both are. No `unsafe` is involved: the JavaScript work never leaves
    /// the one thread the wasm module runs on, and the future the driver awaits is a plain
    /// channel receive.
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, BridgeError>> + Send {
        let this = self.clone();
        let (sender, receiver) =
            futures_channel::oneshot::channel::<Result<Response, BridgeError>>();

        spawn_local(async move {
            // The receiver may have been dropped if the caller cancelled the query; there is
            // nothing useful to do with that, and the fetch itself cannot be aborted here.
            let _ = sender.send(this.post(request).await);
        });

        async move {
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err(BridgeError::Transport(
                    "the fetch task ended before it replied".to_owned(),
                )),
            }
        }
    }
}

/// Turn a JavaScript exception into a transport error.
fn js_error(value: JsValue) -> BridgeError {
    BridgeError::Transport(match value.as_string() {
        Some(text) => text,
        None => format!("{value:?}"),
    })
}
