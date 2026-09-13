//! Every route a module declares must be guarded, or be named as anonymous.
//!
//! `Authorized<P>` makes it impossible to *reach* a caller without the policy
//! having passed — that is a compile-time guarantee and needs no test. What it
//! cannot do is stop someone writing a handler that never asks for a caller at
//! all: a route whose handler takes only `State` compiles, routes, and serves
//! the data to anyone. The type system has nothing to object to, because
//! nothing was claimed.
//!
//! So this reads each module's route table out of its own source and checks
//! that every handler behind it names a guard. A new route is picked up because
//! the inventory comes from the route table itself, not from a list kept
//! alongside it.
//!
//! # What this is and is not
//!
//! It is a syntactic check over source text, not a proof. A handler could name
//! a guard and ignore it, and a route registered somewhere other than a
//! module's `routes()` function would not be seen. What it does catch is the
//! failure it was written for: a route added without a guard.
//!
//! It is written to **fail closed**. A module whose source cannot be found, a
//! route whose path is not a literal, a handler whose definition cannot be
//! located, an allowlist entry naming a route that no longer exists — each of
//! those fails the test rather than passing quietly. A parser that silently
//! found nothing would be worse than no test at all.
//!
//! The health endpoints do not appear here: they come from
//! `shared_kernel::health::routes`, which every module merges in, and they are
//! anonymous in the original too.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The crates that declare their own routes, and the name each mounts under.
///
/// `module-reporting` is absent on purpose: it has no `endpoints.rs` at all,
/// serving only the shared health routes. A test below pins that, so giving it
/// an endpoint surface without adding it here fails.
const ROUTED_MODULES: [(&str, &str); 4] = [
    ("module-admin", "Administration"),
    ("module-identity", "Identity"),
    ("module-music", "Music"),
    ("module-orders", "Orders"),
];

/// A module that deliberately declares no routes of its own.
const HEALTH_ONLY_MODULE: &str = "module-reporting";

/// The routes that are anonymous by design, with the reason each one is.
///
/// Every entry has to be defensible out loud, which is the point of writing
/// them down rather than letting an unguarded handler pass unremarked. All
/// three are in Identity, and all three are anonymous in the original:
/// a caller cannot hold a token before they have logged in.
const ANONYMOUS_ROUTES: [(&str, &str, &str); 3] = [
    (
        "Identity",
        "/login",
        "a caller cannot present a token before they have one",
    ),
    (
        "Identity",
        "/refresh",
        "the refresh token is the credential; the expired access token is not checked",
    ),
    (
        "Identity",
        "/.well-known/jwks.json",
        "public key material, published for anyone validating a token",
    ),
];

/// The smallest number of routes the parser must find before its answers mean
/// anything.
///
/// A parser that quietly matched nothing would report every module clean. This
/// is well below the current count — it is a floor, not an inventory.
const MINIMUM_ROUTES_FOUND: usize = 30;

// ---------------------------------------------------------------------------
// The route inventory
// ---------------------------------------------------------------------------

/// One route, and the handler sites that serve it.
#[derive(Debug)]
struct Route {
    module: &'static str,
    path: String,
    /// One per handler on the route — `put(a).delete(b)` has two.
    handlers: Vec<Handler>,
}

/// A handler, and the parameter list that decides whether it is guarded.
#[derive(Debug)]
struct Handler {
    /// The function's name, or `<closure>`.
    name: String,
    parameters: String,
}

fn workspace_root() -> PathBuf {
    let mut current = Path::new(env!("CARGO_MANIFEST_DIR"));

    loop {
        let manifest = current.join("Cargo.toml");
        if manifest.is_file()
            && std::fs::read_to_string(&manifest).is_ok_and(|text| text.contains("[workspace]"))
        {
            return current.to_path_buf();
        }

        current = current
            .parent()
            .expect("the workspace root should be an ancestor of this crate");
    }
}

fn endpoints_source(module: &str) -> String {
    let path = workspace_root()
        .join("crates")
        .join(module)
        .join("src/endpoints.rs");

    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "`{}` should be readable — a module that stopped declaring its routes there \
             would slip past this test entirely: {error}",
            path.display()
        )
    })
}

/// Every route the four modules declare.
fn route_inventory() -> Vec<Route> {
    ROUTED_MODULES
        .into_iter()
        .flat_map(|(crate_name, module)| {
            let source = endpoints_source(crate_name);
            let guard_aliases = guard_aliases(&source);
            let body = routes_function_body(&source, module);

            routes_in(&body, module, &source, &guard_aliases)
        })
        .collect()
}

/// The body of the module's `routes()` function.
///
/// Scanning only this keeps the `collection` helper's own two `.route` calls —
/// whose paths are computed rather than literal — out of the inventory, without
/// having to special-case them.
fn routes_function_body(source: &str, module: &str) -> String {
    let signature = source
        .find("fn routes")
        .unwrap_or_else(|| panic!("{module} should declare a `routes` function"));

    let opening = source[signature..]
        .find('{')
        .map(|offset| signature + offset)
        .unwrap_or_else(|| panic!("{module}'s `routes` function should have a body"));

    balanced(source, opening, '{', '}')
        .unwrap_or_else(|| panic!("{module}'s `routes` function body should be balanced"))
        .to_owned()
}

/// Collects the routes declared in one `routes()` body.
fn routes_in(
    body: &str,
    module: &'static str,
    source: &str,
    aliases: &BTreeSet<String>,
) -> Vec<Route> {
    let mut routes = Vec::new();

    for arguments in call_arguments(body, ".route(") {
        let parts = split_top_level(&arguments);
        assert!(
            parts.len() >= 2,
            "{module}: a `.route` call should take a path and a method router, got `{arguments}`"
        );

        let path = string_literal(&parts[0]).unwrap_or_else(|| {
            panic!(
                "{module}: `{}` is not a literal route path. Routes are read from source here, \
                 so a computed path cannot be checked — register it literally, or teach this \
                 test how to resolve it",
                parts[0].trim()
            )
        });

        routes.push(Route {
            module,
            path,
            handlers: handlers_in(&parts[1..].join(","), module, source),
        });
    }

    // The `collection(router, "/albums", all_albums)` helper, which registers
    // the slashed and unslashed spellings of one collection route.
    for arguments in call_arguments(body, "collection(") {
        let parts = split_top_level(&arguments);
        if parts.len() != 3 {
            continue;
        }

        let Some(path) = string_literal(&parts[1]) else {
            continue;
        };

        // Unlike `.route`, the helper takes the handler bare rather than
        // wrapped in `get(…)`.
        routes.push(Route {
            module,
            path,
            handlers: vec![named_handler(parts[2].trim(), module, source)],
        });
    }

    assert!(
        !routes.is_empty(),
        "{module}: no routes were parsed out of its `routes()` function, which means this \
         test is reading something it does not understand rather than finding nothing wrong"
    );

    let _ = aliases;
    routes
}

/// The handler sites inside a method-router expression.
fn handlers_in(expression: &str, module: &str, source: &str) -> Vec<Handler> {
    let mut handlers = Vec::new();

    // A closure, as Identity's routes use: `post(move |caller: Authorized<…>, …| {`.
    if let Some(parameters) = closure_parameters(expression) {
        handlers.push(Handler {
            name: "<closure>".to_owned(),
            parameters,
        });
    }

    // A named handler: `get(album_by_id)`, or both halves of `put(a).delete(b)`.
    for name in named_handlers(expression) {
        handlers.push(named_handler(&name, module, source));
    }

    assert!(
        !handlers.is_empty(),
        "{module}: no handler was found in `{}`, so this route's guard cannot be checked",
        expression.trim()
    );

    handlers
}

/// Resolves a routed function name to its parameter list.
fn named_handler(name: &str, module: &str, source: &str) -> Handler {
    assert!(
        is_plain_identifier(name),
        "{module}: `{name}` is routed but is not a plain function name, so its guard \
         cannot be checked"
    );

    let parameters = function_parameters(source, name).unwrap_or_else(|| {
        panic!(
            "{module}: `{name}` is routed but its definition was not found in the same \
             file, so whether it is guarded cannot be established"
        )
    });

    Handler {
        name: name.to_owned(),
        parameters,
    }
}

// ---------------------------------------------------------------------------
// Guard detection
// ---------------------------------------------------------------------------

/// The local type aliases that stand for an `Authorized<…>`.
///
/// The modules alias theirs — `type Caller = Authorized<MusicRead>;`,
/// `type Reader`, `type Writer` — so an alias has to count as a guard or every
/// handler would look unguarded.
fn guard_aliases(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("type ")?;
            let (name, definition) = rest.split_once('=')?;

            definition
                .contains("Authorized<")
                .then(|| name.trim().to_owned())
        })
        .collect()
}

/// Whether a parameter list asks for an authorized caller.
fn names_a_guard(parameters: &str, aliases: &BTreeSet<String>) -> bool {
    if parameters.contains("Authorized<") {
        return true;
    }

    aliases
        .iter()
        .any(|alias| contains_identifier(parameters, alias))
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Whether this route is one of the three that are anonymous by design.
fn is_anonymous_by_design(route: &Route) -> bool {
    ANONYMOUS_ROUTES
        .iter()
        .any(|(module, path, _)| *module == route.module && *path == route.path)
}

#[test]
fn every_module_route_is_guarded_or_named_as_anonymous() {
    let routes = route_inventory();

    let mut unguarded = Vec::new();

    for route in &routes {
        if is_anonymous_by_design(route) {
            continue;
        }

        let aliases = guard_aliases(&endpoints_source(
            ROUTED_MODULES
                .iter()
                .find(|(_, module)| *module == route.module)
                .expect("the route came from one of these modules")
                .0,
        ));

        for handler in &route.handlers {
            if !names_a_guard(&handler.parameters, &aliases) {
                unguarded.push(format!(
                    "  {} {} -> {}({})",
                    route.module,
                    route.path,
                    handler.name,
                    handler
                        .parameters
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
        }
    }

    assert!(
        unguarded.is_empty(),
        "these routes reach a handler that never asks for an authorized caller:\n{}\n\n\
         Take an `Authorized<P>` — naming the policy the route carries — or, if the route \
         really should be open to anyone, add it to ANONYMOUS_ROUTES in this file with the \
         reason it is.",
        unguarded.join("\n")
    );
}

#[test]
fn the_data_modules_permit_no_anonymous_routes_at_all() {
    // Every allowance is in Identity, where a caller cannot yet hold a token.
    // Music, Orders and Administration serve data, and nothing there is open.
    for (module, path, _) in ANONYMOUS_ROUTES {
        assert_eq!(
            module, "Identity",
            "`{module} {path}` is allowed to be anonymous, but only Identity has a reason \
             to be — a data route must never be on this list"
        );
    }
}

#[test]
fn every_anonymous_allowance_still_names_a_route_that_exists() {
    // A stale entry is how an allowlist rots into a blanket exemption: the
    // route is renamed, the entry stops matching anything, and nobody notices
    // it is still there waiting to excuse the next route that takes the name.
    let routes = route_inventory();

    for (module, path, reason) in ANONYMOUS_ROUTES {
        assert!(
            routes
                .iter()
                .any(|route| route.module == module && route.path == path),
            "ANONYMOUS_ROUTES names `{module} {path}` ({reason}), but no such route is \
             declared any more — drop the entry rather than leaving it to excuse something else"
        );
    }
}

#[test]
fn the_parser_found_enough_routes_to_be_believed() {
    let routes = route_inventory();

    assert!(
        routes.len() >= MINIMUM_ROUTES_FOUND,
        "only {} routes were parsed, below the floor of {MINIMUM_ROUTES_FOUND}. A parser that \
         silently stops matching reports every module clean, so this fails instead.",
        routes.len()
    );

    for (_, module) in ROUTED_MODULES {
        assert!(
            routes.iter().any(|route| route.module == module),
            "no routes were found for {module}"
        );
    }
}

#[test]
fn every_guarded_handler_names_a_policy_rather_than_a_bare_caller() {
    // `Authorized<P>` is only a guard because `P` names requirements. A guard
    // parameterised over nothing would not be one, so pin that each resolves to
    // a concrete policy type.
    let routes = route_inventory();
    let mut checked = 0_usize;

    for route in &routes {
        if is_anonymous_by_design(route) {
            continue;
        }

        let source = endpoints_source(
            ROUTED_MODULES
                .iter()
                .find(|(_, module)| *module == route.module)
                .expect("the route came from one of these modules")
                .0,
        );

        for handler in &route.handlers {
            let policy = policy_named_by(&handler.parameters, &source);

            assert!(
                policy.is_some(),
                "{} {} -> {} names a guard, but the policy behind it could not be resolved",
                route.module,
                route.path,
                handler.name
            );
            checked += 1;
        }
    }

    // Every route that is not on the allowlist contributed at least one
    // handler, so anything less means handlers were skipped rather than checked.
    let guarded_routes = routes.len() - ANONYMOUS_ROUTES.len();
    assert!(
        checked >= guarded_routes,
        "only {checked} handlers were checked across {guarded_routes} guarded routes"
    );
}

#[test]
fn the_health_only_module_still_declares_no_routes_of_its_own() {
    // Reporting serves nothing but the shared health endpoints. If it grows an
    // endpoint surface, it has to join ROUTED_MODULES to be checked at all —
    // and this is what says so.
    let path = workspace_root()
        .join("crates")
        .join(HEALTH_ONLY_MODULE)
        .join("src/endpoints.rs");

    assert!(
        !path.exists(),
        "`{HEALTH_ONLY_MODULE}` has grown an `endpoints.rs`. Add it to ROUTED_MODULES so its \
         routes are checked like every other module's."
    );
}

/// The policy type behind a guarded parameter list, resolving one alias hop.
fn policy_named_by(parameters: &str, source: &str) -> Option<String> {
    if let Some(policy) = generic_argument(parameters, "Authorized<") {
        return Some(policy);
    }

    // `_caller: Caller`, where `type Caller = Authorized<MusicRead>;`
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("type ") else {
            continue;
        };
        let Some((name, definition)) = rest.split_once('=') else {
            continue;
        };

        let name = name.trim();
        if !definition.contains("Authorized<") || !contains_identifier(parameters, name) {
            continue;
        }

        if let Some(policy) = generic_argument(definition, "Authorized<") {
            return Some(policy);
        }
    }

    None
}

/// The single generic argument of `prefix`, e.g. `MusicRead` from `Authorized<MusicRead>`.
fn generic_argument(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let end = text[start..].find('>')? + start;

    let argument = text[start..end].trim();
    (!argument.is_empty()).then(|| argument.to_owned())
}

// ---------------------------------------------------------------------------
// A small, deliberately dull Rust scanner
// ---------------------------------------------------------------------------

/// The argument text of every call to `opening` in `source`.
///
/// `opening` includes its own `(`, so `".route("` matches `.route(` and not
/// `my_route(`.
fn call_arguments(source: &str, opening: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut search_from = 0;

    while let Some(offset) = source[search_from..].find(opening) {
        let call = search_from + offset;
        let paren = call + opening.len() - 1;

        match balanced(source, paren, '(', ')') {
            Some(inner) => arguments.push(inner.to_owned()),
            None => break,
        }

        search_from = paren + 1;
    }

    arguments
}

/// The text between `open` at `index` and its matching `close`.
///
/// Skips over string literals and line comments so a brace inside either does
/// not throw the count off.
fn balanced(source: &str, index: usize, open: char, close: char) -> Option<&str> {
    let bytes: Vec<char> = source.chars().collect();
    let offsets: Vec<usize> = source.char_indices().map(|(offset, _)| offset).collect();

    let start = offsets.iter().position(|offset| *offset == index)?;
    if bytes.get(start) != Some(&open) {
        return None;
    }

    let mut depth = 0_usize;
    let mut position = start;

    while position < bytes.len() {
        match bytes[position] {
            '/' if bytes.get(position + 1) == Some(&'/') => {
                while position < bytes.len() && bytes[position] != '\n' {
                    position += 1;
                }
                continue;
            }
            '"' => {
                position += 1;
                while position < bytes.len() {
                    match bytes[position] {
                        '\\' => position += 2,
                        '"' => break,
                        _ => position += 1,
                    }
                }
            }
            character if character == open => depth += 1,
            character if character == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[offsets[start] + open.len_utf8()..offsets[position]]);
                }
            }
            _ => {}
        }

        position += 1;
    }

    None
}

/// Splits an argument list on its top-level commas.
fn split_top_level(arguments: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0_i32;
    let mut characters = arguments.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '/' if characters.peek() == Some(&'/') => {
                for skipped in characters.by_ref() {
                    if skipped == '\n' {
                        break;
                    }
                }
                continue;
            }
            '"' => {
                current.push(character);
                while let Some(inner) = characters.next() {
                    current.push(inner);
                    match inner {
                        '\\' => {
                            if let Some(escaped) = characters.next() {
                                current.push(escaped);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }

        current.push(character);
    }

    if !current.trim().is_empty() {
        parts.push(current);
    }

    parts
}

/// The contents of a `"…"` literal, if that is all this argument is.
fn string_literal(argument: &str) -> Option<String> {
    let trimmed = argument.trim();
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?;

    (!inner.contains('"')).then(|| inner.to_owned())
}

/// The identifiers passed to `get`, `post`, `put`, `delete` and friends.
fn named_handlers(expression: &str) -> Vec<String> {
    const METHODS: [&str; 7] = ["get", "post", "put", "delete", "patch", "head", "options"];

    let mut handlers = Vec::new();

    for method in METHODS {
        let opening = format!("{method}(");
        let mut search_from = 0;

        while let Some(offset) = expression[search_from..].find(&opening) {
            let position = search_from + offset;
            search_from = position + opening.len();

            // `routing::get(` and `my_get(` are not what is being matched.
            let preceded_by_identifier = expression[..position]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
            if preceded_by_identifier {
                continue;
            }

            let Some(inner) = balanced(expression, search_from - 1, '(', ')') else {
                continue;
            };

            let candidate = inner.trim();
            if is_plain_identifier(candidate) {
                handlers.push(candidate.to_owned());
            }
        }
    }

    handlers
}

/// A closure's parameter list, if the expression opens one.
fn closure_parameters(expression: &str) -> Option<String> {
    let opening = expression.find('|')?;
    let closing = expression[opening + 1..].find('|')? + opening + 1;

    Some(expression[opening + 1..closing].to_owned())
}

/// The parameter list of `fn name(…)`, wherever it is defined in the file.
fn function_parameters(source: &str, name: &str) -> Option<String> {
    let needle = format!("fn {name}(");
    let position = source.find(&needle)?;
    let paren = position + needle.len() - 1;

    balanced(source, paren, '(', ')').map(ToOwned::to_owned)
}

fn is_plain_identifier(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
        && !candidate.chars().next().is_some_and(char::is_numeric)
}

/// Whether `needle` appears in `haystack` as a whole identifier.
fn contains_identifier(haystack: &str, needle: &str) -> bool {
    let mut search_from = 0;

    while let Some(offset) = haystack[search_from..].find(needle) {
        let start = search_from + offset;
        let end = start + needle.len();

        let before_is_word = haystack[..start]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');
        let after_is_word = haystack[end..]
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || character == '_');

        if !before_is_word && !after_is_word {
            return true;
        }

        search_from = end;
    }

    false
}
