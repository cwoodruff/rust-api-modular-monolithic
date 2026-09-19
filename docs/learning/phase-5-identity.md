# Phase 5 — The Identity module

*Learning Rust through the port, part 6 of 10.*

| | |
|---|---|
| **What this phase built** | RS256 token issuance and validation, the development key provider, the in-memory user and refresh-token stores, the five auth endpoints, the authentication layer, and the policy guards every later module uses. |
| **Commit** | `6b92398` — `git show 6b92398 --stat` (hardened by `c1c6305`, `94a83e5`, `a0cb0a3`, `0108081`) |
| **Files to open** | `crates/module-identity/src/{lib,options,claims,keys,tokens,stores,runtime,endpoints}.rs`, `crates/shared-kernel/src/auth.rs`, `crates/api/tests/identity.rs` |
| **Rust ideas introduced** | Crypto crates (`rsa`, `jsonwebtoken`, `base64`, `sha2`), a trait with two implementations sharing a private struct, `serde(untagged)`, custom extractors via `FromRequestParts`, `PhantomData`, associated consts, unit structs as type-level policies, `Deref`, atomic operations and `DashMap::remove` as a single-use token, threads and `Barrier` in tests, match guards |

This is the phase with the most security-relevant code, and it is where the
port's habit of writing its own `Debug` impls pays off. It is also the phase
that introduces the single most Rust-specific design in the whole port: the
`Authorized<P>` extractor, which makes a handler *unable to run* unless its
policy passed.

---

## 1. Claims that had to be captured, not read

`claims.rs` opens with the story: the C# host clears the inbound claim-type
map but not the outbound one, so `name`, `email` and `role` are written as
full XML Schema URIs. Reading the source would give the short names; only a
captured token shows the truth. The struct encodes it:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    pub sub: String,
    pub jti: String,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub iss: String,
    /// The audience — emitted **twice**, as an array.
    pub aud: Vec<String>,

    #[serde(rename = "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    // ...
    #[serde(rename = "http://schemas.microsoft.com/ws/2008/06/identity/claims/role")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub roles: Option<ClaimValues>,
}
```

`default` on a field tells serde to use `Default` when the member is absent
on *deserialization*, which pairs with `skip_serializing_if` on the way out.

**A claim that is a string or an array**:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClaimValues {
    One(String),
    Many(Vec<String>),
}
```

`untagged` means serde tries each variant in order with no discriminator: a
JSON string matches `One`, an array matches `Many`. This is how the port
reproduces V3 from the plan — one role serializes as `"Admin"`, two as
`["Admin", "Manager"]` — and reads both back. `from_values` collapses a `Vec`
into the right variant with a `match` on `values.len()`.

---

## 2. Key material: one trait, two providers, one shared core

`keys.rs` defines the port of `IKeyMaterialService`:

```rust
pub trait KeyMaterial: Send + Sync {
    fn signing_key(&self) -> &EncodingKey;
    fn validation_key(&self) -> &DecodingKey;
    fn current_key_id(&self) -> &str;
    fn jwks(&self) -> serde_json::Value;
}
```

Two types implement it — `DevKeyMaterial` (generates and persists a key, in
the C# file format) and `PemKeyMaterial` (loads a supplied PEM; fix F15). Both
derive their four values the same way, so a private struct does the shared
work:

```rust
struct RsaMaterial {
    encoding: EncodingKey,
    decoding: DecodingKey,
    modulus: String,
    exponent: String,
}

impl RsaMaterial {
    fn from_private_key(private: &RsaPrivateKey) -> Result<Self, KeyError> {
        let pem = private.to_pkcs8_pem(LineEnding::LF).map_err(...)?;
        let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).map_err(...)?;
        let public = RsaPublicKey::from(private);
        let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
        let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
        let decoding = DecodingKey::from_rsa_components(&modulus, &exponent).map_err(...)?;
        Ok(Self { encoding, decoding, modulus, exponent })
    }
}
```

This is composition where C# would use a base class: each provider *has* an
`RsaMaterial` and forwards to it. Rust has no inheritance, and this is what
you do instead.

Crate vocabulary: `rsa` for key generation and PKCS#1/PKCS#8 encoding (the
`DecodePrivateKey`/`EncodePrivateKey` traits must be imported for their
methods to appear); `jsonwebtoken` for JWT signing and validation;
`base64::Engine` with the `STANDARD` and `URL_SAFE_NO_PAD` engines (the
trait must be in scope for `.encode`); `sha2` for the RFC 7638 thumbprint.

**Failure messages that do not quote the secret.** `PemKeyMaterial::from_pem`
maps a parse error to `KeyError::Encoding(format!("... not a PKCS#8 or PKCS#1
RSA private key: {error}"))`, and the test
`text_that_is_not_a_key_is_refused_without_quoting_it_back` asserts the input
bytes never appear. When you write error messages around secrets in Rust, the
`Display` output is what ends up in logs; treat it accordingly.

`KeyError` is another `thiserror` enum, with `Unavailable` and `Misconfigured`
variants that carry a `String` explaining what to configure. `runtime.rs`'s
`build_keys` is where those messages are written, and the tests check that
each one names the setting that would fix it.

---

## 3. The token service

`tokens.rs` issues and validates. Issuance builds an `AccessTokenClaims`,
sets the header's `kid`, and signs:

```rust
let mut header = Header::new(Algorithm::RS256);
header.kid = Some(self.keys.current_key_id().to_owned());
let access_token = jsonwebtoken::encode(&header, &claims, self.keys.signing_key())?;
```

Validation configures `jsonwebtoken::Validation` to mirror the C#
`TokenValidationParameters` — issuer, audience, 30 seconds of leeway, and a
`required_spec_claims` set that includes `sub` so a subject-less token fails
inside the library rather than in an afterthought:

```rust
match decoded {
    Ok(data) if !data.claims.sub.trim().is_empty() => Some(data.claims.into_user()),
    Ok(_) => { tracing::warn!("rejected a token with no subject"); None }
    Err(error) => { tracing::debug!(%error, "token validation failed"); None }
}
```

The `Ok(data) if ...` arm is a match guard; the plain `Ok(_)` below it catches
what the guard rejected.

**Single-use refresh tokens (fix F10).** The doc comment on `refresh` is the
best short explanation of a check-then-act race in the repository. The cure
is in `stores.rs`:

```rust
pub fn take(&self, user_id: &str, token: &str) -> bool {
    self.tokens
        .remove(&Self::key(user_id, token))
        .is_some_and(|(_, expires)| expires > Utc::now())
}
```

`DashMap::remove` returns the entry to exactly one caller. The test
`only_one_of_many_racing_callers_can_spend_a_token` spawns sixteen OS threads
behind a `std::sync::Barrier` so they all call `take` at once, and counts
winners with an `AtomicUsize`. It is a compact model for writing a race test
in Rust.

**Sweeping without a timer (fix F12).** `purge_if_due` uses
`AtomicI64::compare_exchange` so that of many concurrent writers only one
claims the sweep. `Ordering::Relaxed` is enough because the timestamp is a
hint, not a synchronization point. This is the same "at most once per
interval, off the request path" shape as Phase 4's rate limiter sweep,
implemented with an atomic instead of a mutex.

**Redaction, again.** `TokenPair`, `UserRecord`, `InMemoryUserStore`,
`InMemoryRefreshTokenStore`, `LoginRequest`, `RefreshRequest`,
`LogoutRequest` and `TokenResponse` all write their own `Debug`. The store's
comment explains the sharpest case: it is keyed by `"{userId}:{token}"`, so
a derived `Debug` would print every live credential in the process.
`finish_non_exhaustive()` renders a trailing `..` to signal fields were left
out.

---

## 4. The authentication layer

`lib.rs` holds the port of `UseAuthentication()`:

```rust
pub async fn authenticate(
    axum::extract::State(runtime): axum::extract::State<Arc<IdentityRuntime>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request.headers().get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(tokens::strip_bearer)
        .map(ToOwned::to_owned);

    if let Some(token) = token
        && let Some(user) = runtime.tokens().validate(&token)
    {
        request.extensions_mut().insert(user);
    }

    next.run(request).await
}
```

It only *populates* the principal; it never rejects. That is the original's
model (endpoints are anonymous unless they opt in), and it is what makes the
guard in the next section the single place a refusal is decided.

The `if let ... && let ...` chain binds two `Option`s in one condition. The
`mut request` parameter is needed because `extensions_mut()` mutates it —
Rust parameters are immutable unless declared `mut`.

---

## 5. `Authorized<P>`: the check you cannot forget

This is in `shared-kernel/src/auth.rs`, and its doc comments say why the
guards live in the kernel: the C# source has a note saying they should move
to `SharedKernel`, and the port made that move so no module depends on
Identity.

### 5a. Policies as types

```rust
pub trait Policy: Send + Sync + 'static {
    const REQUIREMENTS: &'static [Requirement];
    const NAME: &'static str;
}

pub mod guards {
    #[derive(Debug, Clone, Copy)]
    pub struct MusicRead;

    impl Policy for MusicRead {
        const REQUIREMENTS: &'static [Requirement] = &[
            Requirement::Permission(policies::MUSIC_READ),
            Requirement::TenantScope,
        ];
        const NAME: &'static str = policies::MUSIC_READ;
    }
    // Authenticated, OrdersRead, AdministrationRead, AdministrationWrite
}
```

**Associated consts** are data attached to a trait implementation rather than
to an instance. `MusicRead` is a **unit struct** — a type with no fields and
one value — used only as a type-level label. There is no runtime object; the
policy is chosen by naming the type.

### 5b. The extractor

```rust
pub struct Authorized<P: Policy> {
    user: AuthenticatedUser,
    request_tenant: Option<String>,
    policy: PhantomData<fn() -> P>,
}

impl<S, P> FromRequestParts<S> for Authorized<P>
where
    S: Send + Sync,
    P: Policy,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let principal = Principal::from_request_parts(parts, state).await?;
        match principal.authorize(P::REQUIREMENTS) {
            Ok(user) => Ok(Self { user: user.clone(), request_tenant: principal.request_tenant.clone(), policy: PhantomData }),
            Err(failure) => {
                tracing::debug!(policy = P::NAME, ?failure, "authorization refused");
                Err(failure.into())
            }
        }
    }
}
```

- **`FromRequestParts`** is axum's trait for extractors that only need the
  request head (headers, extensions, URI). Implementing it is what lets a
  handler write `caller: Authorized<MusicRead>` as a parameter.
- **`type Rejection = ApiError`** says what happens when extraction fails:
  axum calls `into_response` on the rejection and never runs the handler. So
  a 401 or 403 is produced *by the type*, before the handler body exists.
- **`PhantomData<fn() -> P>`** records the policy type without storing a
  value. The `fn() -> P` form (rather than `PhantomData<P>`) is a standard
  trick to avoid imposing `P`'s `Send`/`Sync`/lifetime properties on the
  struct; here it just says "we mention `P`, we do not own one."
- **`for_test`** is the only constructor besides the extractor, and it is
  named so nobody mistakes it for a general one. The doc says why: outside
  tests the extractor is the only way to get one, which is the whole point.

The type-level lesson: **make the illegal state unrepresentable.** A handler
that wants a caller must ask for `Authorized<P>`, and the only way to obtain
one is for every requirement to have passed. The commit that introduced this
(`c1c6305`) replaced a `refuse(&principal)` line at the top of each handler —
a line review could forget. What the type still cannot catch is a handler
that never asks for a caller at all; Phase 7's `route_guards.rs` test covers
that hole.

### 5c. `Deref` for ergonomics

```rust
impl<P: Policy> Deref for Authorized<P> {
    type Target = AuthenticatedUser;
    fn deref(&self) -> &Self::Target { &self.user }
}
```

With this, `caller.subject` works as if `Authorized` were the user. `Deref`
is what makes `String` act like `&str` and `Arc<T>` act like `T`; using it
for a thin wrapper like this is idiomatic, using it to fake inheritance is
not.

### 5d. The requirements themselves

`Principal::authorize` loops over `&[Requirement]` and matches each variant.
`tenant_scope_satisfied` ports the surprising half of the original's tenant
handler and is documented in place: a caller who sends no `X-Tenant-Id`
succeeds, scoped implicitly; a caller with no tenant *claim* always fails.
The unit tests enumerate every branch, and the `Authorized` tests at the end
of the file build a one-route router and drive it with `oneshot` to prove
the extractor and the loop agree.

---

## 6. The endpoints

`endpoints.rs` shows three handler return styles side by side:

- `handle_login` returns a bare `Response`, because it has three distinct
  outcomes (400 problem, 401 without a challenge, 200 envelope) and builds
  each explicitly. Note `unauthorized()` from the kernel — the version
  *without* `WWW-Authenticate`, which Phase 8's diff showed was required.
- `handle_logout` returns `Result<StatusCode, ApiError>`, so `?`-style
  early returns render as problem documents.
- `handle_userinfo` returns `Json<UserInfoResponse>` and takes
  `Authorized<Authenticated>` as its only argument — the smallest possible
  guarded handler.

The routes capture an `Arc<IdentityRuntime>` per closure:

```rust
let login = Arc::clone(&runtime);
// ...
.route("/login", post(move |Json(request): Json<LoginRequest>| {
    let runtime = Arc::clone(&login);
    async move { handle_login(&runtime, request) }
}))
```

Each closure must own its handle (`move`), and the inner `Arc::clone` is
needed because the closure may be called many times while the `async move`
block consumes what it captures. This is the manual version of what
`State<Arc<IdentityRuntime>>` would do; the module keeps its runtime out of
the shared `AppState` on purpose, as `composition.rs`'s doc explains.

`format_offset` is another hand-rolled .NET format: a `DateTimeOffset` writes
`+00:00` rather than `Z` and trims trailing zeros from the fraction, and V4 in
the plan is the capture that proved it.

---

## 7. Assembly and environment gating

`runtime.rs`'s `build_keys` is a `match` on the provider name with **match
guards** doing case-insensitive comparison:

```rust
match options.key_provider.trim() {
    provider if provider.eq_ignore_ascii_case("File") || provider.eq_ignore_ascii_case("Pem") => { ... }
    provider if provider.eq_ignore_ascii_case("Environment") || ... => { ... }
    provider if provider.eq_ignore_ascii_case("KeyVault") => Err(KeyError::Unavailable(...)),
    provider if provider.is_empty() || provider.eq_ignore_ascii_case("Dev") => { /* refused outside Dev/Demo */ }
    other => Err(KeyError::Misconfigured(format!("unsupported Jwt:KeyProvider value '{other}'; ..."))),
}
```

A `match` on a `&str` with guards is how you write a case-insensitive
dispatch table. The final `other =>` arm binds whatever did not match and
uses it in the message.

`build_user_store` returns `Arc<dyn UserStore>` and chooses
`DisabledUserStore` outside Development and Demo, so configured credentials
cannot be used in production even if present — the same gate the original
has.

---

## 8. Two bugs only running it found

The commit message records them, and both are lessons about tests:

- Environment-variable arrays did not bind (Phase 1's config fix), so the
  host started with no logins. The unit tests passed because they built
  configs from JSON.
- `traceId` was degenerate — both halves came from a counter. The test
  checked segment *lengths* and passed. It now checks the values vary
  (`trace_identifiers_actually_vary_across_their_whole_width` in Phase 1's
  `errors.rs`).

The integration tests in `crates/api/tests/identity.rs` are the port of the
C# identity suite, and their doc comment says the expected shapes were
captured from the running service. They build the app with
`IdentityRuntime::with_keys` and a freshly generated key so no key file is
written into the working tree.

---

## Try it

1. **Add a policy.** Define `pub struct ReportView;` in `guards` with
   `Requirement::Permission(policies::REPORT_VIEW)`, then write a one-route
   router in a test (copy `guarded::<P>()`) and check a user without the
   permission gets 403.
2. **Remove the guard and see what compiles.** In `handle_userinfo`, change
   the parameter to `Principal` and try to read `caller.subject`. The
   compiler shows you why `Authorized` is the safer type.
3. **Race the refresh token.** Change `TokenService::refresh` to call
   `validate` then `revoke` instead of `take`, run
   `cargo test -p module-identity racing_refreshes`, and watch sixteen
   winners appear. Revert.
4. **Print a token.** In a scratch test, `println!("{pair:?}")` for a
   `TokenPair`, then temporarily `#[derive(Debug)]` it and print again.
5. **Decode what you issued.** Log in against a running host, paste the
   access token into any JWT decoder, and compare the claim names with
   `claims.rs`. Note the doubled `aud`.
6. **Start Production on a PEM.** `openssl genrsa -out /tmp/k.pem 2048`, then
   `ASPNETCORE_ENVIRONMENT=Production Jwt__KeyProvider=File Jwt__PemKeyPath=/tmp/k.pem cargo run -p api`
   and fetch `/api/identity/.well-known/jwks.json`. Restart and confirm the
   `kid` is identical (RFC 7638 thumbprint).

## Vocabulary for this phase

| C# / .NET | Rust | Notes |
|---|---|---|
| `ClaimsPrincipal` | `AuthenticatedUser` | Plain struct |
| `RequireAuthorization("policy")` on a route | `caller: Authorized<Policy>` on the handler | Enforced by the extractor |
| `IAuthorizationHandler` | `Principal::authorize` + `Requirement` enum | |
| Policy name string | Unit struct implementing `Policy` | Associated consts |
| Abstract base class | Trait + a shared private struct | Composition, not inheritance |
| `JsonConverter` for string-or-array | `#[serde(untagged)]` enum | |
| `RSA.Create(2048)` | `RsaPrivateKey::new(&mut rng, 2048)` | `rsa` crate |
| `JwtSecurityTokenHandler` | `jsonwebtoken::{encode, decode, Validation}` | |
| `Convert.ToBase64String` | `base64::engine::general_purpose::STANDARD.encode` | Trait must be in scope |
| `ConcurrentDictionary.TryRemove` | `DashMap::remove` → `Option<(K, V)>` | Atomic take |
| `Interlocked.CompareExchange` | `AtomicI64::compare_exchange` | With memory orderings |
| `Barrier` / `Task.WhenAll` for a race test | `std::sync::Barrier` + `std::thread::spawn` | |
| Implicit conversion operator | `impl Deref` (for wrappers) / `impl From` | |
| Generic type parameter with no field | `PhantomData<fn() -> P>` | Zero-sized |
| `switch` with `StringComparison.OrdinalIgnoreCase` | `match s { x if x.eq_ignore_ascii_case("...") => ... }` | |

**Next:** [Phase 6 — Music and Orders](phase-6-music-orders.md): the first
data modules, the cache-aside services, and the parity harness that found
four bugs.
