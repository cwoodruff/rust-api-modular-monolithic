//! A JSON body extractor that fails the way the original fails.
//!
//! axum's own `Json` rejection writes a plain-text body, which is not what the
//! C# host does. Its exception handler turns a `JsonException` into a 400
//! problem document titled `Malformed request.` with the parser's message as
//! the detail, and a `BadHttpRequestException` — a missing or wrong content
//! type, for instance — into the same shape.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::ProblemDetails;
use crate::errors::new_trace_id;

/// A JSON request body.
///
/// Behaves like [`axum::Json`] on success and like the C# host on failure.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = MalformedRequest;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(MalformedRequest::from(rejection)),
        }
    }
}

/// A body the host could not read.
#[derive(Debug, Clone)]
pub struct MalformedRequest {
    status: StatusCode,
    detail: String,
}

impl From<JsonRejection> for MalformedRequest {
    fn from(rejection: JsonRejection) -> Self {
        // The original answers 400 for a parse failure and lets a
        // `BadHttpRequestException` carry its own status — 415 for an
        // unsupported content type, for instance.
        Self {
            status: rejection.status(),
            detail: rejection.body_text(),
        }
    }
}

impl MalformedRequest {
    /// The problem document this becomes.
    #[must_use]
    pub fn into_problem(self) -> ProblemDetails {
        ProblemDetails::malformed_request(self.status, self.detail, new_trace_id())
    }
}

impl IntoResponse for MalformedRequest {
    fn into_response(self) -> Response {
        self.into_problem().into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, header};
    use axum::routing::post;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Debug, Deserialize)]
    struct Payload {
        name: String,
    }

    fn app() -> Router {
        Router::new().route(
            "/",
            post(|JsonBody(payload): JsonBody<Payload>| async move { payload.name }),
        )
    }

    async fn post_body(content_type: Option<&str>, body: &'static str) -> (StatusCode, String) {
        let mut builder = HttpRequest::builder().method("POST").uri("/");
        if let Some(value) = content_type {
            builder = builder.header(header::CONTENT_TYPE, value);
        }

        let response = app()
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap();

        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn a_well_formed_body_is_accepted() {
        let (status, body) = post_body(Some("application/json"), r#"{"name":"Rock"}"#).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "Rock");
    }

    #[tokio::test]
    async fn unparseable_json_answers_the_originals_malformed_problem() {
        let (status, body) = post_body(Some("application/json"), "{not json").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);

        let document: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(document["title"], "Malformed request.");
        assert_eq!(document["status"], 400);
        assert_eq!(
            document["type"],
            "https://www.rfc-editor.org/rfc/rfc9110#section-15.5.1"
        );
        assert!(
            document["detail"].as_str().is_some_and(|d| !d.is_empty()),
            "the parser's message is carried as the detail"
        );
        assert!(document["traceId"].is_string());
    }

    #[tokio::test]
    async fn a_body_missing_a_required_member_is_also_malformed() {
        let (status, body) = post_body(Some("application/json"), r#"{"other":1}"#).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let document: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(document["title"], "Malformed request.");
    }

    #[tokio::test]
    async fn a_missing_content_type_carries_its_own_status() {
        // The original's BadHttpRequestException keeps whatever status it was
        // raised with rather than flattening everything to 400.
        let (status, body) = post_body(None, r#"{"name":"Rock"}"#).await;

        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let document: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(document["title"], "Malformed request.");
        assert_eq!(document["status"], 415);
    }
}
