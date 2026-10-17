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
use crate::errors::{ApiError, current_trace_id};

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
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::Malformed(MalformedRequest::from(rejection))),
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
    ///
    /// Only a body the parser choked on gets the `Malformed request.` shape.
    /// Anything else — an unsupported content type, most commonly — never
    /// reaches the C# exception handler at all: it is a bodiless status that
    /// the problem-details middleware fills in from its defaults table.
    /// Verified against the running service, where a request with no
    /// `Content-Type` answers `Unsupported Media Type` rather than
    /// `Malformed request.`.
    #[must_use]
    pub fn into_problem(self) -> ProblemDetails {
        let trace_id = current_trace_id();

        self.into_problem_with(trace_id)
    }

    /// The problem document this becomes, under an explicit trace identifier.
    #[must_use]
    pub fn into_problem_with(self, trace_id: impl Into<String>) -> ProblemDetails {
        if self.status == StatusCode::BAD_REQUEST {
            ProblemDetails::malformed_request(self.status, self.detail, trace_id)
        } else {
            crate::errors::status_code_page(self.status, trace_id)
        }
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
    async fn a_missing_content_type_answers_the_defaults_table_document() {
        // This never reaches the C# exception handler: it is a bodiless status
        // the problem-details middleware fills in, so it reads `Unsupported
        // Media Type` rather than `Malformed request.`.
        let (status, body) = post_body(None, r#"{"name":"Rock"}"#).await;

        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let document: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(document["title"], "Unsupported Media Type");
        assert_eq!(document["status"], 415);
        assert_eq!(
            document["type"],
            "https://tools.ietf.org/html/rfc9110#section-15.5.16"
        );
        assert!(document.get("detail").is_none());
    }
}

/// Timestamps rendered the way `System.Text.Json` renders them.
///
/// A UTC `DateTime` is `2007-01-02T00:00:00Z`; a `DateTimeOffset` is
/// `2026-10-16T02:00:00.1234567+00:00`. Both carry up to seven fractional
/// digits with trailing zeros removed — not chrono's three-six-nine — and a
/// whole second carries none. Use with `#[serde(with = "…")]`.
pub mod dotnet {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    fn render(instant: &DateTime<Utc>, suffix: &str) -> String {
        let ticks = instant.timestamp_subsec_nanos() / 100;
        let base = instant.format("%Y-%m-%dT%H:%M:%S");

        if ticks == 0 {
            return format!("{base}{suffix}");
        }

        let fraction = format!("{ticks:07}");
        format!("{base}.{}{suffix}", fraction.trim_end_matches('0'))
    }

    /// A UTC `DateTime`: `…Z`.
    #[must_use]
    pub fn date_time(instant: &DateTime<Utc>) -> String {
        render(instant, "Z")
    }

    /// A `DateTimeOffset` at UTC: `…+00:00`.
    #[must_use]
    pub fn date_time_offset(instant: &DateTime<Utc>) -> String {
        render(instant, "+00:00")
    }

    fn parse<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|instant| instant.with_timezone(&Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
                    .map(|naive| naive.and_utc())
            })
            .map_err(serde::de::Error::custom)
    }

    /// `#[serde(with = "dotnet::decimal")]` for a `Decimal` written as .NET writes one.
    ///
    /// `System.Text.Json` writes a decimal with its own scale: `3.96`, `1.00`,
    /// and `0` for `0m`. The workspace serializes decimals as floats, which
    /// agrees on every value with a fraction and turns `0m` into `0.0`. A
    /// scale-zero decimal is written as an integer here, so a total no event
    /// has added to reads `0`, as upstream's does.
    pub mod decimal {
        use rust_decimal::Decimal;
        use rust_decimal::prelude::ToPrimitive;
        use serde::Serializer;

        /// Serializes.
        ///
        /// # Errors
        ///
        /// The serializer's.
        pub fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
            match (value.scale(), value.to_i64()) {
                (0, Some(whole)) => serializer.serialize_i64(whole),
                _ => serializer.serialize_f64(value.to_f64().unwrap_or_default()),
            }
        }
    }

    /// `#[serde(with = "dotnet::utc")]` for a `DateTime<Utc>` field written as a `DateTime`.
    pub mod utc {
        use super::{DateTime, Deserializer, Serializer, Utc};

        /// Serializes.
        ///
        /// # Errors
        ///
        /// The serializer's.
        pub fn serialize<S: Serializer>(
            instant: &DateTime<Utc>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&super::date_time(instant))
        }

        /// Deserializes, treating a timestamp with no zone as UTC as upstream's converter does.
        ///
        /// # Errors
        ///
        /// Text that is not a timestamp.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<DateTime<Utc>, D::Error> {
            super::parse(deserializer)
        }
    }

    /// The same, for an optional field.
    pub mod utc_option {
        use super::{DateTime, Deserialize, Deserializer, Serializer, Utc};

        /// Serializes, writing `null` for `None`.
        ///
        /// # Errors
        ///
        /// The serializer's.
        pub fn serialize<S: Serializer>(
            instant: &Option<DateTime<Utc>>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match instant {
                Some(instant) => serializer.serialize_str(&super::date_time(instant)),
                None => serializer.serialize_none(),
            }
        }

        /// Deserializes.
        ///
        /// # Errors
        ///
        /// Text that is not a timestamp.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            let text: Option<String> = Option::deserialize(deserializer)?;
            text.map(|text| {
                super::parse(serde::de::value::StringDeserializer::<D::Error>::new(text))
            })
            .transpose()
        }
    }

    /// `#[serde(with = "dotnet::offset")]` for a field written as a `DateTimeOffset`.
    pub mod offset {
        use super::{DateTime, Deserializer, Serializer, Utc};

        /// Serializes.
        ///
        /// # Errors
        ///
        /// The serializer's.
        pub fn serialize<S: Serializer>(
            instant: &DateTime<Utc>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&super::date_time_offset(instant))
        }

        /// Deserializes.
        ///
        /// # Errors
        ///
        /// Text that is not a timestamp.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<DateTime<Utc>, D::Error> {
            super::parse(deserializer)
        }
    }

    /// The same, for an optional field.
    pub mod offset_option {
        use super::{DateTime, Deserialize, Deserializer, Serializer, Utc};

        /// Serializes, writing `null` for `None`.
        ///
        /// # Errors
        ///
        /// The serializer's.
        pub fn serialize<S: Serializer>(
            instant: &Option<DateTime<Utc>>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match instant {
                Some(instant) => serializer.serialize_str(&super::date_time_offset(instant)),
                None => serializer.serialize_none(),
            }
        }

        /// Deserializes.
        ///
        /// # Errors
        ///
        /// Text that is not a timestamp.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            let text: Option<String> = Option::deserialize(deserializer)?;
            text.map(|text| {
                super::parse(serde::de::value::StringDeserializer::<D::Error>::new(text))
            })
            .transpose()
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used)]

        use super::*;

        #[test]
        fn fractions_trim_to_what_dotnet_writes() {
            let instant = DateTime::parse_from_rfc3339("2026-10-16T02:00:00.120000Z")
                .unwrap()
                .with_timezone(&Utc);

            assert_eq!(date_time(&instant), "2026-10-16T02:00:00.12Z");
            assert_eq!(date_time_offset(&instant), "2026-10-16T02:00:00.12+00:00");
        }

        #[test]
        fn a_scale_zero_decimal_is_written_as_an_integer() {
            #[derive(serde::Serialize)]
            struct Total(#[serde(with = "super::decimal")] rust_decimal::Decimal);

            assert_eq!(
                serde_json::to_string(&Total(rust_decimal::Decimal::ZERO)).unwrap(),
                "0"
            );
            assert_eq!(
                serde_json::to_string(&Total(rust_decimal::Decimal::new(396, 2))).unwrap(),
                "3.96"
            );
        }

        #[test]
        fn a_whole_second_has_no_fraction() {
            let instant = DateTime::parse_from_rfc3339("2007-01-02T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc);

            assert_eq!(date_time(&instant), "2007-01-02T00:00:00Z");
        }
    }
}
