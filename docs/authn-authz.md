# Authentication and authorization

**Status: implemented**, except the Key Vault provider — the trait is in place
and the development provider is complete. That matches the original's own
position: its Phase 4 is "in-memory stores implemented; persistent stores and
KMS integration pending".

## Tokens

RS256, RSA-2048, fifteen-minute access tokens, seven-day opaque refresh tokens.

The claim names were **captured from a token the original issued**, not read
off its source, and that mattered. The host clears
`JwtSecurityTokenHandler.DefaultInboundClaimTypeMap` but leaves the *outbound*
map alone, so three claims go out as full XML Schema URIs rather than the short
names the `ClaimTypes.Name` spelling suggests:

| Claim | On the wire |
|---|---|
| name | `http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name` |
| email | `http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress` |
| role | `http://schemas.microsoft.com/ws/2008/06/identity/claims/role` |
| tenant | `tenant` |
| permissions | `permissions` |

Two more things the capture settled:

- **`aud` is emitted twice**, as an array. `JwtSecurityToken`'s constructor
  adds the audience and the claim list already carries one.
- **A repeated claim collapses.** One role serializes as a string, two or more
  as an array. Any reader has to accept both forms.

Guessing any of these would have produced tokens the original rejects. The port
was then checked the other way: same seeded login, same claim keys, same values
for everything but `jti`, `iat`, `nbf` and `exp`.

Validation checks issuer, audience, lifetime and signature with **30 seconds**
of clock skew, accepts `typ` of `at+jwt` or `JWT`, tolerates a doubled
`Bearer Bearer ` prefix, and rejects a token with no `sub`. Note the original's
`authn-authz-plan.md` says two minutes of skew; the code says thirty seconds,
and the code is what ships.

## Refresh

Refresh rotates: the old token is revoked on use, so a second attempt with the
same token answers 401. Permissions and tenant are **re-read from the user
store** on refresh, so a revoked permission cannot ride along into the new
token, and a user who has since been removed has their token revoked and gets
nothing back.

The store is process-local and lost on restart, and it never evicts expired
entries — it only checks expiry on read. Both are true of the original.

## Logins

The in-memory store is enabled **only in Development and Demo**. Anywhere else
a disabled store takes over and refuses every login, so configured credentials
cannot be used in production even if they are present.

There are **no seeded users**. The original ships `"InMemoryUsers": []` and
documents `dotnet user-secrets` for filling it; this port ships the same empty
list. Supply logins through configuration or the environment:

```sh
export Identity__InMemoryUsers__0__Username=demo
export Identity__InMemoryUsers__0__Password=secret
export Identity__InMemoryUsers__0__UserId=user-1
export Identity__InMemoryUsers__0__Tenant=tenant-1
export Identity__InMemoryUsers__0__Roles__0=User
export Identity__InMemoryUsers__0__Permissions__0=music.read
```

Passwords are compared in plain text. Usernames match case-insensitively.
Entries missing a username, password or user id are dropped with a warning
rather than failing startup. All four behaviors are the original's.

## Keys

The development provider generates an RSA-2048 key on first use and persists it
as `{"Kid": …, "PrivateKeyPkcs8Base64": …}` — the same file the C# provider
writes, so an existing dev key loads unchanged. It refuses to run outside
Development and Demo, exactly as the original refuses, with a message naming
what to configure instead.

**Unlike the original, the key is not committed.** The C# repository has a real
RSA private key in git; this one is generated on first run and gitignored (F6).

JWKS is served from `/api/identity/.well-known/jwks.json` — inside the module
group, not at the conventional root path. That is the original's arrangement,
it is wire-visible, and `/.well-known/jwks.json` correctly answers 404. One key
is published, with no rotation overlap.

## Policies

Ten of them. Eight are permission checks, which are just the permission string:

`music.read`, `music.write`, `orders.read`, `orders.write`,
`admin.users.manage`, `administration.read`, `administration.write`,
`report.view`

plus `role.admin` and `tenant.scoped`.

**Four reach no endpoint** — `music.write`, `orders.write`,
`admin.users.manage` and `report.view` — and are kept unused here for the same
reason the original keeps them: the documentation refers to them.

There is **no global fallback policy**. An endpoint is anonymous unless it opts
in, which is why every health endpoint is reachable without a token even inside
otherwise protected modules.

Requirements stack, and all must pass:

| Endpoints | Requirements |
|---|---|
| Music data | `music.read` + `tenant.scoped` |
| Orders data | `orders.read` + `tenant.scoped` |
| Administration reads | `role.admin` + `administration.read` + `tenant.scoped` |
| Administration writes | `role.admin` + `administration.write` + `tenant.scoped` |
| `logout`, `userinfo` | authenticated |
| health, data-health, root, login, refresh, JWKS | anonymous |

## The tenant guard, which surprises people

Two halves, and the second is the one to read twice:

- A caller with **no tenant claim** always fails, whatever they asked for.
- A caller who names **no tenant** — no `X-Tenant-Id` header — **succeeds**,
  scoped implicitly to their own tenant.

So isolation is enforced only when the caller volunteers the header. The
original's own comment explains the intent: it "allows Swagger/curl usage
without the header while still enforcing tenant isolation when the header IS
provided". Worth knowing before relying on it as a boundary, which is why this
port keeps it rather than quietly tightening it.

When a tenant *is* named, the comparison is ordinal — `TENANT-1` does not match
`tenant-1`.

The resolver also checks route values `tenant` and `tenantId` before the
header, but no route template in the application declares either, so only the
header can ever match.

## 401 versus 403

Both appear, and which one you get is not arbitrary:

- **401 with `WWW-Authenticate: Bearer`** — a protected endpoint reached
  without a usable token. The header comes from the authentication challenge.
- **401 with no challenge header** — an endpoint decided this itself, such as
  a rejected login. `Results.Unauthorized()` from a handler.
- **403** — authenticated, but a requirement failed.

That distinction was found by diffing headers against the running service; it
is easy to get wrong in a way no body comparison would catch.
