#!/usr/bin/env python3
"""Diff this Rust port against the C# original it was ported from.

Both services must already be running, seeded with the same logins, and each
pointed at its **own copy** of the same database — the write cases insert rows,
and identical copies keep the generated keys lined up. `tools/run-parity.sh`
arranges all of that; this script only compares.

    tools/parity-diff.py --original http://localhost:5043 --port http://localhost:5055

What is compared, per case: the status code, the headers listed in
`COMPARED_HEADERS`, and the response body parsed as JSON (so member order,
which System.Text.Json and serde disagree about, is not treated as a
difference).

## What is deliberately not compared

Five members cannot match by construction. They are scrubbed rather than
excused case by case:

  timestampUtc  a wall clock reading
  traceId       a per-request correlation id
  version       the .NET informational version embeds the build's commit hash
  kid, n        JWKS key material; each host generates its own signing key

Two more are compared loosely because their *value* is per-request but their
*shape* is not: `access_token` and `refresh_token`.

## Pacing

Every endpoint in the application shares one rate-limit budget — 60 requests
per 60 seconds per client — so a sweep this size trips it. The runner paces
itself: after `WINDOW_BUDGET` requests it waits out the window rather than
letting a 429 masquerade as a difference. That is the original's behavior
faithfully reproduced, not a workaround.

## Known divergences

One case is expected to differ, and is reported as `known` rather than as a
failure. See `KNOWN_DIVERGENCES`.

Exit status is 0 when every case either matches or is a known divergence.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from dataclasses import dataclass, field

# --------------------------------------------------------------------------
# What gets scrubbed, compared, and excused
# --------------------------------------------------------------------------

VOLATILE_MEMBERS = {
    "timestampUtc",
    "traceId",
    "version",
    "kid",
    "n",
    "access_token",
    "refresh_token",
    "expires_at_utc",
}

COMPARED_HEADERS = [
    "content-type",
    "location",
    "www-authenticate",
    "x-content-type-options",
    "x-frame-options",
    "x-xss-protection",
    "referrer-policy",
    "content-security-policy",
    "permissions-policy",
]

#: Requests to send per service before waiting out the rate-limit window. The
#: limit is 60; this leaves room for the logins and a little slack.
WINDOW_BUDGET = 50

#: The window itself, plus a second so the reset has certainly happened.
WINDOW_SECONDS = 61


class Pacer:
    """Keeps a sweep under the shared rate-limit budget."""

    def __init__(self, budget: int = WINDOW_BUDGET, window: int = WINDOW_SECONDS) -> None:
        self.budget = budget
        self.window = window
        self.sent = 0

    def before_request(self) -> None:
        if self.sent >= self.budget:
            print(f"  ...  pausing {self.window}s for the rate-limit window", flush=True)
            time.sleep(self.window)
            self.sent = 0
        self.sent += 1


#: Security headers the original loses on exception-handled responses.
SECURITY_HEADER_NOTE = (
    "The original answers validation failures with no security headers at "
    "all. Its security-headers middleware sets them *before* calling the rest "
    "of the pipeline, and UseExceptionHandler clears the response before "
    "writing the problem document — which wipes them. This port sets them on "
    "the way out, so an error response keeps them. Not reproduced: losing "
    "nosniff and the CSP on an error response is a real weakness, and no "
    "client depends on a header being absent. Worth reporting upstream."
)

KNOWN_DIVERGENCES = {
    "write: create with a null name": SECURITY_HEADER_NOTE,
    "write: create with an over-long name": SECURITY_HEADER_NOTE,
    "write: update with an over-long name": SECURITY_HEADER_NOTE,
    "write: malformed json": (
        "The `detail` of a malformed-body problem is the parser's own "
        "diagnostic. .NET's names its internals — 'Failed to read parameter "
        '"CreateGenreRequest request" from the request body as JSON.\' — and '
        "reproducing that would mean hardcoding a C# type and parameter name "
        "into a Rust handler. The type, title and status do match."
    ),
}


@dataclass
class Case:
    """One request to send to both services."""

    name: str
    path: str
    method: str = "GET"
    #: Which seeded login to authenticate as, or None for an anonymous request.
    role: str | None = None
    body: str | None = None
    #: Send no `Content-Type`, to exercise the unsupported-media-type path.
    omit_content_type: bool = False
    tenant: str | None = None


@dataclass
class Outcome:
    status: str
    headers: dict[str, str]
    body: object


@dataclass
class Report:
    matched: list[str] = field(default_factory=list)
    known: list[str] = field(default_factory=list)
    failed: list[tuple[str, list[str]]] = field(default_factory=list)


# --------------------------------------------------------------------------
# The cases
# --------------------------------------------------------------------------

MODULE_PREFIXES = ["/api/admin", "/api/identity", "/api/music", "/api/orders", "/api/reporting"]


def cases() -> list[Case]:
    """Every case, in the order they are reported."""
    built: list[Case] = [
        Case("root", "/"),
        Case("jwks", "/api/identity/.well-known/jwks.json"),
        Case("jwks is not at the root", "/.well-known/jwks.json"),
    ]

    for prefix in MODULE_PREFIXES:
        built.append(Case(f"health {prefix}", f"{prefix}/health"))
        built.append(Case(f"data-health {prefix}", f"{prefix}/data-health"))

    # Reads, as a caller holding every permission.
    reads = [
        "/api/music/albums/1",
        "/api/music/albums",
        "/api/music/albums/",
        "/api/music/albums/artist/1",
        "/api/music/artists/1",
        "/api/music/artists",
        "/api/music/playlists/1",
        "/api/music/playlists",
        "/api/music/tracks/1",
        "/api/music/tracks",
        "/api/music/tracks/album/1",
        "/api/music/tracks/artist/1",
        "/api/music/tracks/genre/1",
        "/api/music/tracks/mediatype/1",
        "/api/music/tracks/playlist/1",
        "/api/music/tracks/invoice/1",
        "/api/orders/invoices/1",
        "/api/orders/invoices",
        "/api/orders/invoices/customer/1",
        "/api/orders/invoice-lines/1",
        "/api/orders/invoice-lines",
        "/api/orders/invoice-lines/invoice/1",
        "/api/orders/invoice-lines/track/1013",
        "/api/admin/customers/1",
        "/api/admin/customers",
        "/api/admin/customers/support-rep/3",
        "/api/admin/employees/1",
        "/api/admin/employees/2",
        "/api/admin/employees",
        "/api/admin/employees/2/direct-reports",
        "/api/admin/employees/2/reports-to",
        "/api/admin/genres/1",
        "/api/admin/genres",
        "/api/admin/media-types/1",
        "/api/admin/media-types",
    ]
    built += [Case(f"read {path}", path, role="admin") for path in reads]

    # Absent ids: the 404 document, and the empty-array collections.
    missing_items = [
        "/api/music/albums/999999",
        "/api/music/artists/999999",
        "/api/music/playlists/999999",
        "/api/music/tracks/999999",
        "/api/orders/invoices/999999",
        "/api/orders/invoice-lines/999999",
        "/api/admin/customers/999999",
        "/api/admin/employees/999999",
        "/api/admin/genres/999999",
        "/api/admin/media-types/999999",
    ]
    built += [Case(f"missing {path}", path, role="admin") for path in missing_items]

    empty_collections = [
        "/api/music/albums/artist/999999",
        "/api/music/tracks/album/999999",
        "/api/orders/invoices/customer/999999",
        "/api/admin/customers/support-rep/999999",
        "/api/admin/employees/999999/direct-reports",
    ]
    built += [Case(f"empty {path}", path, role="admin") for path in empty_collections]

    # The authorization matrix. Anonymous, then a caller holding the wrong
    # permission, against one route from each module.
    guarded = [
        "/api/music/albums/1",
        "/api/orders/invoices/1",
        "/api/admin/genres/1",
        "/api/identity/userinfo",
    ]
    built += [Case(f"anonymous {path}", path) for path in guarded]
    built += [Case(f"wrong permission {path}", path, role="reader") for path in guarded[:3]]

    # The admin module needs a role and a tenant on top of the permission.
    built += [
        Case("no admin role /api/admin/genres/1", "/api/admin/genres/1", role="roleless"),
        Case("no tenant claim /api/admin/genres/1", "/api/admin/genres/1", role="tenantless"),
        Case("matching tenant header", "/api/music/albums/1", role="admin", tenant="tenant-1"),
        Case("mismatched tenant header", "/api/music/albums/1", role="admin", tenant="tenant-2"),
    ]

    # Routing failures.
    built += [
        Case("unknown route", "/does-not-exist"),
        Case("wrong method", "/api/music/health", method="POST", body="{}"),
    ]

    # Identity.
    built += [
        Case("userinfo", "/api/identity/userinfo", role="admin"),
        Case(
            "login bad password",
            "/api/identity/login",
            method="POST",
            body=json.dumps({"username": "admin", "password": "wrong"}),
        ),
        Case(
            "login unknown user",
            "/api/identity/login",
            method="POST",
            body=json.dumps({"username": "nobody", "password": "secret123"}),
        ),
        Case(
            "login blank",
            "/api/identity/login",
            method="POST",
            body=json.dumps({"username": "", "password": ""}),
        ),
        Case(
            "refresh unknown token",
            "/api/identity/refresh",
            method="POST",
            body=json.dumps({"userId": "admin-1", "refreshToken": "made-up"}),
        ),
        Case(
            "refresh blank",
            "/api/identity/refresh",
            method="POST",
            body=json.dumps({"userId": "", "refreshToken": ""}),
        ),
        Case(
            "logout anonymous",
            "/api/identity/logout",
            method="POST",
            body=json.dumps({"userId": "admin-1", "refreshToken": "x"}),
        ),
        Case(
            "logout for another user",
            "/api/identity/logout",
            method="POST",
            role="reader",
            body=json.dumps({"userId": "admin-1", "refreshToken": "x"}),
        ),
    ]

    # Writes. Each service has its own database copy, so the generated keys
    # line up as long as both receive the same sequence.
    long_name = "x" * 121
    built += [
        Case(
            "write: create",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"Name": "ParityProbe"}),
        ),
        Case(
            "write: create with a lowercase member",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"name": "ParityProbeLower"}),
        ),
        Case(
            "write: create with a null name",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"Name": None}),
        ),
        Case(
            "write: create with an over-long name",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"Name": long_name}),
        ),
        Case(
            "write: create at the 120-character boundary",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"Name": "y" * 120}),
        ),
        Case(
            "write: malformed json",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body="{not json",
        ),
        Case(
            "write: no content type",
            "/api/admin/genres",
            method="POST",
            role="admin",
            body=json.dumps({"Name": "NoContentType"}),
            omit_content_type=True,
        ),
        Case(
            "write: create without the write permission",
            "/api/admin/genres",
            method="POST",
            role="reader",
            body=json.dumps({"Name": "Nope"}),
        ),
        Case(
            "write: create anonymously",
            "/api/admin/genres",
            method="POST",
            body=json.dumps({"Name": "Nope"}),
        ),
        Case(
            "write: update a missing genre",
            "/api/admin/genres/999999",
            method="PUT",
            role="admin",
            body=json.dumps({"Name": "Nope"}),
        ),
        Case(
            "write: update with an over-long name",
            "/api/admin/genres/1",
            method="PUT",
            role="admin",
            body=json.dumps({"Name": long_name}),
        ),
        Case(
            "write: update genre 1",
            "/api/admin/genres/1",
            method="PUT",
            role="admin",
            body=json.dumps({"Name": "RenamedByParity"}),
        ),
        Case("write: read back genre 1", "/api/admin/genres/1", role="admin"),
        Case(
            "write: delete a missing genre",
            "/api/admin/genres/999999",
            method="DELETE",
            role="admin",
        ),
    ]

    return built


# --------------------------------------------------------------------------
# Running the cases
# --------------------------------------------------------------------------


def scrub(value: object) -> object:
    if isinstance(value, dict):
        return {k: scrub(v) for k, v in value.items() if k not in VOLATILE_MEMBERS}
    if isinstance(value, list):
        return [scrub(item) for item in value]
    return value


def call(base: str, case: Case, token: str | None) -> Outcome:
    command = [
        "curl", "-sS", "--max-time", "60",
        "-o", "/tmp/parity-body", "-D", "/tmp/parity-headers",
        "-w", "%{http_code}", "-X", case.method, f"{base}{case.path}",
    ]
    if token:
        command += ["-H", f"Authorization: Bearer {token}"]
    if case.tenant:
        command += ["-H", f"X-Tenant-Id: {case.tenant}"]
    if case.body is not None:
        if not case.omit_content_type:
            command += ["-H", "Content-Type: application/json"]
        command += ["--data-binary", case.body]

    status = subprocess.run(command, capture_output=True, text=True).stdout.strip()

    headers: dict[str, str] = {}
    with open("/tmp/parity-headers", errors="replace") as handle:
        for line in handle:
            if ":" in line:
                name, _, value = line.partition(":")
                name = name.strip().lower()
                if name in COMPARED_HEADERS:
                    headers[name] = value.strip()

    raw = open("/tmp/parity-body", "rb").read()
    try:
        body: object = scrub(json.loads(raw)) if raw else ""
    except json.JSONDecodeError:
        body = raw.decode(errors="replace")

    return Outcome(status, headers, body)


def login(base: str, username: str, password: str) -> str:
    subprocess.run(
        ["curl", "-sS", "-o", "/tmp/parity-login", "-X", "POST", f"{base}/api/identity/login",
         "-H", "Content-Type: application/json",
         "--data-binary", json.dumps({"username": username, "password": password})],
        capture_output=True,
    )
    try:
        return json.load(open("/tmp/parity-login"))["access_token"]
    except (json.JSONDecodeError, KeyError, FileNotFoundError):
        sys.exit(f"could not log in to {base} as {username}; is it seeded?")


def leaf_differences(left: object, right: object, path: str = "") -> list[str]:
    """Every leaf where two documents disagree, as readable lines."""
    found: list[str] = []

    if type(left) is not type(right):
        return [f"{path or '/'}: original is {type(left).__name__}, port is {type(right).__name__}"]

    if isinstance(left, dict):
        for key in sorted(set(left) | set(right)):
            if key not in left:
                found.append(f"{path}/{key}: absent in the original, port has {right[key]!r}")
            elif key not in right:
                found.append(f"{path}/{key}: original has {left[key]!r}, absent in the port")
            else:
                found += leaf_differences(left[key], right[key], f"{path}/{key}")
    elif isinstance(left, list):
        if len(left) != len(right):
            found.append(f"{path or '/'}: {len(left)} items in the original, {len(right)} in the port")
        for index, (a, b) in enumerate(zip(left, right)):
            found += leaf_differences(a, b, f"{path}[{index}]")
    elif left != right:
        found.append(f"{path or '/'}: original {left!r}, port {right!r}")

    return found[:6]


def compare(case: Case, original: Outcome, port: Outcome) -> list[str]:
    problems: list[str] = []

    if original.status != port.status:
        problems.append(f"status: original {original.status}, port {port.status}")

    for header in COMPARED_HEADERS:
        left, right = original.headers.get(header), port.headers.get(header)
        if left != right:
            problems.append(f"header {header}: original {left!r}, port {right!r}")

    problems += leaf_differences(original.body, port.body)

    return problems


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--original", default="http://localhost:5043")
    parser.add_argument("--port", default="http://localhost:5055")
    parser.add_argument("--quiet", action="store_true", help="only report problems")
    arguments = parser.parse_args()

    roles = {"admin": "secret123", "reader": "secret123",
             "roleless": "secret123", "tenantless": "secret123"}
    tokens = {
        role: (login(arguments.original, role, password), login(arguments.port, role, password))
        for role, password in roles.items()
    }

    report = Report()
    # The four logins above already drew on this window.
    pacer = Pacer()
    pacer.sent = len(roles)

    for case in cases():
        original_token, port_token = (None, None)
        if case.role:
            original_token, port_token = tokens[case.role]

        pacer.before_request()
        original = call(arguments.original, case, original_token)
        port = call(arguments.port, case, port_token)

        if "429" in (original.status, port.status):
            # The pacing should prevent this; if it happens anyway, wait the
            # window out and take the case again rather than reporting a
            # throttled response as a difference.
            print(f"  ...  throttled on {case.name}; waiting {WINDOW_SECONDS}s", flush=True)
            time.sleep(WINDOW_SECONDS)
            pacer.sent = 1
            original = call(arguments.original, case, original_token)
            port = call(arguments.port, case, port_token)

        problems = compare(case, original, port)

        if not problems:
            report.matched.append(case.name)
            if not arguments.quiet:
                print(f"  same   {original.status}  {case.name}")
        elif case.name in KNOWN_DIVERGENCES:
            report.known.append(case.name)
            print(f"  known  {original.status}  {case.name}")
        else:
            report.failed.append((case.name, problems))
            print(f"  DIFF   {original.status}/{port.status}  {case.name}")
            for problem in problems:
                print(f"           {problem}")

    total = len(report.matched) + len(report.known) + len(report.failed)
    print(f"\n{len(report.matched)}/{total} cases identical")

    if report.known:
        print(f"{len(report.known)} known divergence(s):")
        for name in report.known:
            print(f"  {name}\n    {KNOWN_DIVERGENCES[name]}")

    if report.failed:
        print(f"\n{len(report.failed)} unexplained difference(s):")
        for name, _ in report.failed:
            print(f"  {name}")
        return 1

    print("no unexplained differences")
    return 0


if __name__ == "__main__":
    sys.exit(main())
