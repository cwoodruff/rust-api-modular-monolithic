//! Sending one request through a router and reading the whole answer.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

/// One response, already read.
#[derive(Debug)]
pub struct TestResponse {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    /// The body.
    pub body: Bytes,
}

impl TestResponse {
    /// The body as JSON.
    ///
    /// # Panics
    ///
    /// When it is not JSON.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| panic!("expected JSON, got {:?}: {error}", self.text()))
    }

    /// The body as text.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// One header's value.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|value| value.to_str().ok()).map(ToOwned::to_owned)
    }
}

/// A request under construction.
#[derive(Debug)]
pub struct TestRequest {
    method: Method,
    uri: String,
    headers: Vec<(String, String)>,
    body: Option<(String, String)>,
}

impl TestRequest {
    /// `GET uri`.
    #[must_use]
    pub fn get(uri: &str) -> Self {
        Self::new(Method::GET, uri)
    }

    /// `POST uri`.
    #[must_use]
    pub fn post(uri: &str) -> Self {
        Self::new(Method::POST, uri)
    }

    /// `PUT uri`.
    #[must_use]
    pub fn put(uri: &str) -> Self {
        Self::new(Method::PUT, uri)
    }

    /// `DELETE uri`.
    #[must_use]
    pub fn delete(uri: &str) -> Self {
        Self::new(Method::DELETE, uri)
    }

    /// Any method.
    #[must_use]
    pub fn new(method: Method, uri: &str) -> Self {
        Self {
            method,
            uri: uri.to_owned(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Adds a header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Adds every header in `headers`.
    #[must_use]
    pub fn headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.headers.extend(headers);
        self
    }

    /// `Authorization: Bearer <token>`.
    #[must_use]
    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", &format!("Bearer {token}"))
    }

    /// A JSON body.
    #[must_use]
    pub fn json(mut self, body: &Value) -> Self {
        self.body = Some(("application/json".to_owned(), body.to_string()));
        self
    }

    /// A raw body with its content type.
    #[must_use]
    pub fn raw(mut self, content_type: &str, body: &str) -> Self {
        self.body = Some((content_type.to_owned(), body.to_owned()));
        self
    }

    /// Sends it.
    ///
    /// # Panics
    ///
    /// When the router fails to answer, which a router never should.
    pub async fn send(self, router: &Router) -> TestResponse {
        let mut builder = Request::builder().method(self.method).uri(&self.uri);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }

        let body = match self.body {
            Some((content_type, body)) => {
                builder = builder.header(header::CONTENT_TYPE, content_type);
                Body::from(body)
            }
            None => Body::empty(),
        };

        let response = router.clone().oneshot(builder.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();

        TestResponse { status, headers, body }
    }
}
