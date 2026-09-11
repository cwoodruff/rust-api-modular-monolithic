//! The module contract, ported from the C# `IModule` interface.
//!
//! The original's contract has three members:
//!
//! ```csharp
//! string Name { get; }
//! void RegisterServices(IServiceCollection services, IConfiguration config);
//! void MapEndpoints(IEndpointRouteBuilder endpoints);
//! ```
//!
//! Only two survive the port. `RegisterServices` exists to populate a DI
//! container, and this port has none: the host constructs each module's
//! services directly and hands them over in the shared state, so there is
//! nothing for a registration hook to do. What remains is the module's
//! identity and its routes.
//!
//! [`Module`] is generic over the host's state type rather than naming it,
//! which keeps this crate at the root of the dependency graph — the state is
//! assembled from repositories the shared kernel must not know about.

use axum::Router;

/// One module of the monolith: a name, a mount point, and a set of routes.
pub trait Module<S>: Send + Sync
where
    S: Clone + Send + Sync + 'static,
{
    /// The module's name, as its health endpoint reports it.
    fn name(&self) -> &'static str;

    /// Where the module mounts, such as `/api/music`.
    ///
    /// Written without a trailing slash; [`mount_all`] supplies the separator.
    fn prefix(&self) -> &'static str;

    /// The module's routes, relative to its prefix.
    fn router(&self) -> Router<S>;
}

/// Nests every module under its own prefix.
///
/// Port of the host's `foreach (var module in modules) module.MapEndpoints(app)`.
///
/// # Panics
///
/// Panics if a module declares a prefix axum cannot nest at, such as `/` or one
/// ending in a slash. That is a programming error in the module, and failing at
/// startup beats serving the wrong routes.
pub fn mount_all<S>(mut router: Router<S>, modules: &[Box<dyn Module<S>>]) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    for module in modules {
        router = router.nest(module.prefix(), module.router());
    }

    router
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use axum::routing::get;

    struct TestModule {
        name: &'static str,
        prefix: &'static str,
    }

    impl Module<()> for TestModule {
        fn name(&self) -> &'static str {
            self.name
        }

        fn prefix(&self) -> &'static str {
            self.prefix
        }

        fn router(&self) -> Router<()> {
            Router::new().route("/health", get(|| async { "Healthy" }))
        }
    }

    fn modules() -> Vec<Box<dyn Module<()>>> {
        vec![
            Box::new(TestModule {
                name: "Administration",
                prefix: "/api/admin",
            }),
            Box::new(TestModule {
                name: "Identity",
                prefix: "/api/identity",
            }),
            Box::new(TestModule {
                name: "Music",
                prefix: "/api/music",
            }),
            Box::new(TestModule {
                name: "Orders",
                prefix: "/api/orders",
            }),
            Box::new(TestModule {
                name: "Reporting",
                prefix: "/api/reporting",
            }),
        ]
    }

    #[test]
    fn the_real_prefixes_all_mount_without_conflicting() {
        // axum panics on a prefix it cannot nest at — a trailing slash, or `/`.
        // Mounting all five together also proves none of them collide.
        let _router = mount_all(Router::new(), &modules());
    }

    #[test]
    fn modules_report_their_identity() {
        let modules = modules();

        let names: Vec<&str> = modules.iter().map(|module| module.name()).collect();
        let prefixes: Vec<&str> = modules.iter().map(|module| module.prefix()).collect();

        assert_eq!(
            names,
            ["Administration", "Identity", "Music", "Orders", "Reporting"]
        );
        assert_eq!(
            prefixes,
            [
                "/api/admin",
                "/api/identity",
                "/api/music",
                "/api/orders",
                "/api/reporting"
            ]
        );
    }
}
