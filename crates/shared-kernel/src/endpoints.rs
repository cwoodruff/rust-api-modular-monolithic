//! Mapping endpoints, ported from `IEndpointRouteBuilder`, `MapGroup` and the
//! endpoint conventions upstream chains onto each route.
//!
//! A module maps its routes through a [`RouteGroup`], the counterpart of
//! `app.MapGroup("/api/catalog")`:
//!
//! ```ignore
//! let mut group = endpoints.map_group("/api/catalog", "Catalog", state);
//! group.require_rate_limiting(limiter).add_module_metrics(meter);
//! group
//!     .map_get("/albums/{id}", handlers::get_album_by_id)
//!     .with_name("CatalogGetAlbumById")
//!     .require_authorization(&[permissions::CATALOG_READ, policies::TENANT_SCOPED])
//!     .produces(&[200, 404]);
//! ```
//!
//! Every route is also recorded as an [`EndpointDescriptor`] — upstream's
//! endpoint metadata. The host's startup check, the per-module OpenAPI
//! documents and the guardrail tests all read the descriptors rather than the
//! router, which axum keeps opaque.
//!
//! # Order
//!
//! Upstream's rule, which `EndpointFilterTests` checks: a group's filters run
//! before an endpoint's own, and filters on one builder run in the order they
//! were added. Here, outermost first: the group's rate limit, authorization,
//! the group's filters in order, the endpoint's filters in order, then the
//! handler — whose extractors, [`Validated`](crate::validation::Validated)
//! among them, run last.

use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::handler::Handler;
use axum::http::Method;
use axum::middleware::{self, Next};
use axum::routing::{MethodRouter, delete, get, patch, post, put};
use serde::Serialize;

use crate::auth::{self, PolicyEvaluator};
use crate::diagnostics::{self, ModuleMeter};
use crate::environment::Environment;
use crate::traffic_control::{self, RateLimiter};
use crate::validation::{RequestValidator, ValidatorSlot};

/// A layer applied to one endpoint's method router: upstream's endpoint filter.
pub type Filter = Arc<dyn Fn(MethodRouter) -> MethodRouter + Send + Sync>;

/// Builds a [`Filter`] from a middleware function.
pub fn filter<F, Fut>(middleware: F) -> Filter
where
    F: Fn(Request, Next) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = axum::response::Response> + Send + 'static,
{
    Arc::new(move |router: MethodRouter| {
        let middleware = middleware.clone();
        router.route_layer(middleware::from_fn(move |request: Request, next: Next| {
            let middleware = middleware.clone();
            async move { middleware(request, next).await }
        }))
    })
}

/// What the host tells every module about itself.
#[derive(Debug, Clone)]
pub struct HostInfo {
    /// The environment the host runs as.
    pub environment: Environment,
    /// `ServiceName`, as health bodies report it.
    pub service_name: String,
}

/// One endpoint's metadata. Port of what `EndpointDataSource` exposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndpointDescriptor {
    /// `GET`, `POST`, …
    pub method: &'static str,
    /// The route as declared, prefix included: `/api/catalog/albums/`.
    pub route: String,
    /// The operation name: `CatalogGetAlbumById`.
    pub name: Option<String>,
    /// The OpenAPI tag, which is also the module's document.
    pub tag: &'static str,
    /// Policy names, every one of which must pass.
    pub policies: Vec<&'static str>,
    /// Whether an authenticated caller is required at all.
    pub requires_authorization: bool,
    /// The group's rate-limit policy.
    pub rate_limit_policy: Option<&'static str>,
    /// The OpenAPI description.
    pub description: Option<String>,
    /// Statuses the endpoint can answer, for OpenAPI.
    pub responses: Vec<u16>,
    /// The request type its body is validated as, if any.
    pub validates: Option<&'static str>,
}

impl EndpointDescriptor {
    /// The route as upstream's duplicate check compares it: `/` plus the route
    /// with its slashes trimmed, lowercased.
    #[must_use]
    pub fn normalized_route(&self) -> String {
        format!("/{}", self.route.trim_matches('/')).to_lowercase()
    }
}

/// The conventions chained onto one endpoint.
#[derive(Clone, Default)]
pub struct EndpointConventions {
    name: Option<String>,
    description: Option<String>,
    policies: Vec<&'static str>,
    requires_authorization: bool,
    responses: Vec<u16>,
    filters: Vec<Filter>,
    validates: Option<&'static str>,
}

impl EndpointConventions {
    /// Port of `.WithName(...)`.
    pub fn with_name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// Port of `.WithDescription(...)`.
    pub fn with_description(&mut self, description: impl Into<String>) -> &mut Self {
        self.description = Some(description.into());
        self
    }

    /// Port of `.RequireAuthorization(a).RequireAuthorization(b)`: every policy
    /// must pass. An empty list means "any authenticated caller", upstream's
    /// bare `.RequireAuthorization()`.
    pub fn require_authorization(&mut self, policies: &'static [&'static str]) -> &mut Self {
        self.requires_authorization = true;
        self.policies.extend_from_slice(policies);
        self
    }

    /// Port of `.Produces(status)`.
    pub fn produces(&mut self, statuses: &[u16]) -> &mut Self {
        self.responses.extend_from_slice(statuses);
        self
    }

    /// Port of `.AddEndpointFilter(...)`.
    pub fn add_filter(&mut self, filter: Filter) -> &mut Self {
        self.filters.push(filter);
        self
    }

    /// Port of `.AddEndpointFilter<ValidationFilter<T>>()`.
    ///
    /// The handler takes a [`Validated<T>`](crate::validation::Validated),
    /// which finds this validator and refuses an invalid body with a 400
    /// before the handler's code runs.
    pub fn with_validation<T: Send + 'static>(
        &mut self,
        validator: Arc<dyn RequestValidator<T>>,
    ) -> &mut Self {
        self.validates = Some(
            std::any::type_name::<T>()
                .rsplit("::")
                .next()
                .unwrap_or("T"),
        );
        let slot = ValidatorSlot(validator);
        self.add_filter(Arc::new(move |router: MethodRouter| {
            let slot = slot.clone();
            router.route_layer(middleware::from_fn(
                move |mut request: Request, next: Next| {
                    request.extensions_mut().insert(slot.clone());
                    async move { next.run(request).await }
                },
            ))
        }))
    }
}

/// A route-parameter constraint: `{id:int}`, `{id:guid}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Constraint {
    Int,
    Guid,
}

impl Constraint {
    fn accepts(self, value: &str) -> bool {
        match self {
            Self::Int => value.parse::<i32>().is_ok(),
            Self::Guid => uuid::Uuid::parse_str(value).is_ok(),
        }
    }
}

/// Splits `/albums/{id:int}` into axum's `/albums/{id}` and its constraints.
fn parse_route(route: &str) -> (String, Vec<(String, Constraint)>) {
    let mut constraints = Vec::new();
    let segments: Vec<String> = route
        .split('/')
        .map(|segment| {
            let Some(inner) = segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
            else {
                return segment.to_owned();
            };
            match inner.split_once(':') {
                Some((name, "int")) => {
                    constraints.push((name.to_owned(), Constraint::Int));
                    format!("{{{name}}}")
                }
                Some((name, "guid")) => {
                    constraints.push((name.to_owned(), Constraint::Guid));
                    format!("{{{name}}}")
                }
                _ => segment.to_owned(),
            }
        })
        .collect();

    (segments.join("/"), constraints)
}

/// Answers 404 for a parameter its constraint rejects.
///
/// ASP.NET treats a failed constraint as "no such route", so `/albums/abc`
/// is a 404 before authentication is asked — not a 400, and not a 401.
async fn enforce_constraints(
    constraints: Arc<[(String, Constraint)]>,
    request: Request,
    next: Next,
) -> axum::response::Response {
    use axum::extract::FromRequestParts;
    use axum::response::IntoResponse;

    let (mut parts, body) = request.into_parts();
    let params = axum::extract::RawPathParams::from_request_parts(&mut parts, &()).await;

    if let Ok(params) = params {
        for (name, value) in &params {
            if let Some((_, constraint)) = constraints.iter().find(|(wanted, _)| wanted == name)
                && !constraint.accepts(value)
            {
                return crate::ApiError::NotFound.into_response();
            }
        }
    }

    next.run(Request::from_parts(parts, body)).await
}

struct Pending {
    method: Method,
    path: String,
    router: MethodRouter,
    conventions: EndpointConventions,
}

/// Every endpoint the host serves, as modules map them.
pub struct Endpoints {
    router: Router,
    descriptors: Vec<EndpointDescriptor>,
    evaluator: Arc<dyn PolicyEvaluator>,
    host: HostInfo,
}

impl std::fmt::Debug for Endpoints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Endpoints")
            .field("descriptors", &self.descriptors.len())
            .finish_non_exhaustive()
    }
}

impl Endpoints {
    /// An empty set, deciding policies with `evaluator`.
    #[must_use]
    pub fn new(evaluator: Arc<dyn PolicyEvaluator>, host: HostInfo) -> Self {
        Self {
            router: Router::new(),
            descriptors: Vec::new(),
            evaluator,
            host,
        }
    }

    /// What the host tells modules about itself.
    #[must_use]
    pub fn host(&self) -> &HostInfo {
        &self.host
    }

    /// Port of `MapGroup(prefix)`, with the tag every endpoint in it carries.
    ///
    /// `prefix` is written without a trailing slash; `""` maps at the root.
    pub fn map_group<S>(
        &mut self,
        prefix: &'static str,
        tag: &'static str,
        state: S,
    ) -> RouteGroup<'_, S>
    where
        S: Clone + Send + Sync + 'static,
    {
        RouteGroup {
            endpoints: self,
            prefix,
            tag,
            state,
            rate_limiter: None,
            filters: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Adds routes that carry no endpoint metadata, such as Swagger UI's assets.
    pub fn merge_unlisted(&mut self, router: Router) {
        self.router = std::mem::take(&mut self.router).merge(router);
    }

    /// Every endpoint mapped so far.
    #[must_use]
    pub fn descriptors(&self) -> &[EndpointDescriptor] {
        &self.descriptors
    }

    /// The router and the metadata that describes it.
    pub fn into_parts(self) -> (Router, Vec<EndpointDescriptor>) {
        (self.router, self.descriptors)
    }
}

/// A group of endpoints under one prefix. Port of `RouteGroupBuilder`.
///
/// The group is mapped when it is dropped, which is when upstream's builder
/// would have finished applying its conventions too.
pub struct RouteGroup<'e, S> {
    endpoints: &'e mut Endpoints,
    prefix: &'static str,
    tag: &'static str,
    state: S,
    rate_limiter: Option<RateLimiter>,
    filters: Vec<Filter>,
    pending: Vec<Pending>,
}

impl<S> RouteGroup<'_, S>
where
    S: Clone + Send + Sync + 'static,
{
    /// Port of `.RequireRateLimiting(policy)` on the group: one budget for
    /// every endpoint in it.
    pub fn require_rate_limiting(&mut self, limiter: RateLimiter) -> &mut Self {
        self.rate_limiter = Some(limiter);
        self
    }

    /// Port of `.AddModuleMetrics(module)`.
    pub fn add_module_metrics(&mut self, meter: Arc<ModuleMeter>) -> &mut Self {
        self.add_filter(filter(move |request, next| {
            diagnostics::record_request(Arc::clone(&meter), request, next)
        }))
    }

    /// Port of `.AddEndpointFilter(...)` on the group.
    pub fn add_filter(&mut self, filter: Filter) -> &mut Self {
        self.filters.push(filter);
        self
    }

    /// What the host tells modules about itself.
    #[must_use]
    pub fn host(&self) -> &HostInfo {
        &self.endpoints.host
    }

    /// The group's state, which every handler is given.
    #[must_use]
    pub fn state(&self) -> &S {
        &self.state
    }

    fn map<H, T>(&mut self, method: Method, path: &str, handler: H) -> &mut EndpointConventions
    where
        H: Handler<T, S>,
        T: 'static,
    {
        let router = match method {
            Method::GET => get(handler),
            Method::POST => post(handler),
            Method::PUT => put(handler),
            Method::DELETE => delete(handler),
            _ => patch(handler),
        }
        .with_state(self.state.clone());

        self.pending.push(Pending {
            method,
            path: path.to_owned(),
            router,
            conventions: EndpointConventions::default(),
        });

        let last = self.pending.len() - 1;
        &mut self.pending[last].conventions
    }

    /// Port of `MapGet`.
    ///
    /// A path written with a trailing slash, as upstream writes its collection
    /// routes (`"albums/"`), answers with and without it — ASP.NET's routing
    /// ignores the slash and axum's does not.
    pub fn map_get<H, T>(&mut self, path: &str, handler: H) -> &mut EndpointConventions
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.map(Method::GET, path, handler)
    }

    /// Port of `MapPost`.
    pub fn map_post<H, T>(&mut self, path: &str, handler: H) -> &mut EndpointConventions
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.map(Method::POST, path, handler)
    }

    /// Port of `MapPut`.
    pub fn map_put<H, T>(&mut self, path: &str, handler: H) -> &mut EndpointConventions
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.map(Method::PUT, path, handler)
    }

    /// Port of `MapDelete`.
    pub fn map_delete<H, T>(&mut self, path: &str, handler: H) -> &mut EndpointConventions
    where
        H: Handler<T, S>,
        T: 'static,
    {
        self.map(Method::DELETE, path, handler)
    }
}

impl<S> Drop for RouteGroup<'_, S> {
    fn drop(&mut self) {
        let mut group = Router::new();
        let policy = self.rate_limiter.as_ref().map(RateLimiter::name);

        for pending in std::mem::take(&mut self.pending) {
            let Pending {
                method,
                path,
                mut router,
                conventions,
            } = pending;

            // Innermost first: the endpoint's filters, the group's, then
            // authorization outside them all.
            for endpoint_filter in conventions.filters.iter().rev() {
                router = endpoint_filter(router);
            }
            for group_filter in self.filters.iter().rev() {
                router = group_filter(router);
            }
            if conventions.requires_authorization {
                let evaluator = Arc::clone(&self.endpoints.evaluator);
                let policies: Arc<[&'static str]> = conventions.policies.clone().into();
                router =
                    router.route_layer(middleware::from_fn(move |request: Request, next: Next| {
                        auth::authorize(
                            Arc::clone(&evaluator),
                            Arc::clone(&policies),
                            request,
                            next,
                        )
                    }));
            }

            let (axum_path, constraints) = parse_route(&path);
            if !constraints.is_empty() {
                let constraints: Arc<[(String, Constraint)]> = constraints.into();
                router =
                    router.route_layer(middleware::from_fn(move |request: Request, next: Next| {
                        enforce_constraints(Arc::clone(&constraints), request, next)
                    }));
            }

            let trimmed = axum_path.trim_end_matches('/');
            let canonical = if trimmed.is_empty() { "/" } else { trimmed };
            group = group.route(canonical, router.clone());
            if axum_path.len() > 1 && axum_path.ends_with('/') {
                group = group.route(&axum_path, router);
            }

            self.endpoints.descriptors.push(EndpointDescriptor {
                method: method_name(&method),
                route: format!("{}{}", self.prefix, path),
                name: conventions.name,
                tag: self.tag,
                policies: conventions.policies,
                requires_authorization: conventions.requires_authorization,
                rate_limit_policy: policy,
                description: conventions.description,
                responses: conventions.responses,
                validates: conventions.validates,
            });
        }

        if let Some(limiter) = self.rate_limiter.clone() {
            group = group.layer(middleware::from_fn(move |request: Request, next: Next| {
                traffic_control::rate_limit(limiter.clone(), request, next)
            }));
        }

        let router = std::mem::take(&mut self.endpoints.router);
        self.endpoints.router = if self.prefix.is_empty() {
            router.merge(group)
        } else {
            router.nest(self.prefix, group)
        };
    }
}

fn method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        _ => "PATCH",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constraints_are_stripped_for_axum_and_kept_for_checking() {
        let (path, constraints) = parse_route("/outbox/dead-letters/{id:guid}/retry");

        assert_eq!(path, "/outbox/dead-letters/{id}/retry");
        assert_eq!(constraints, [("id".to_owned(), Constraint::Guid)]);
        assert!(!Constraint::Int.accepts("abc"));
        assert!(Constraint::Int.accepts("42"));
    }
}
