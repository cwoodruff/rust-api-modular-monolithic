#!/usr/bin/env python3
"""Diff the Rust port against the C# original, route by route.

Both services must already be running with the *same* seeded logins and the
*same* database file. Usage:

    tools/parity-diff.py --original http://localhost:5043 --port http://localhost:5055

These members are treated as volatile and scrubbed before comparing:

  timestampUtc  a wall clock reading
  traceId       a per-request identifier
  version       the informational version, which embeds the build's commit
                hash on the .NET side and so can never match by construction
  kid, n        JWKS key material; each host generates its own signing key

Anything else that differs is a real difference.
"""

import argparse
import json
import subprocess
import sys

VOLATILE = {
    "timestampUtc",  # a wall clock reading
    "traceId",       # a per-request identifier
    "version",       # embeds the build's commit hash on the .NET side
    "kid",           # key material: each host generates its own signing key
    "n",             # the modulus of that key
}

ANONYMOUS = [
    "/",
    "/api/admin/health",
    "/api/admin/data-health",
    "/api/identity/health",
    "/api/identity/data-health",
    "/api/music/health",
    "/api/music/data-health",
    "/api/orders/health",
    "/api/orders/data-health",
    "/api/reporting/health",
    "/api/reporting/data-health",
    "/api/identity/.well-known/jwks.json",
]

AUTHENTICATED = [
    "/api/identity/userinfo",
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
    # Absent ids, to compare the 404 documents.
    "/api/music/albums/999999",
    "/api/music/artists/999999",
    "/api/music/playlists/999999",
    "/api/music/tracks/999999",
    "/api/orders/invoices/999999",
    "/api/orders/invoice-lines/999999",
    # Absent ids on collection routes, which answer 200 with an empty array.
    "/api/music/albums/artist/999999",
    "/api/music/tracks/album/999999",
    "/api/orders/invoices/customer/999999",
]


def scrub(value):
    if isinstance(value, dict):
        return {k: scrub(v) for k, v in value.items() if k not in VOLATILE}
    if isinstance(value, list):
        return [scrub(item) for item in value]
    return value


def fetch(base, path, token=None):
    command = ["curl", "-sS", "-o", "/tmp/parity-body", "-w", "%{http_code}", f"{base}{path}"]
    if token:
        command[1:1] = ["-H", f"Authorization: Bearer {token}"]

    status = subprocess.run(command, capture_output=True, text=True).stdout
    body = open("/tmp/parity-body", "rb").read()

    try:
        return status, scrub(json.loads(body))
    except json.JSONDecodeError:
        return status, body.decode(errors="replace")


def login(base, username, password):
    subprocess.run(
        ["curl", "-sS", "-o", "/tmp/parity-token", "-X", "POST", f"{base}/api/identity/login",
         "-H", "Content-Type: application/json",
         "-d", json.dumps({"username": username, "password": password})],
        capture_output=True,
    )
    try:
        return json.load(open("/tmp/parity-token"))["access_token"]
    except (json.JSONDecodeError, KeyError):
        sys.exit(f"could not log in to {base}; is it running with seeded users?")


def differences(left, right, path="", found=None):
    """Every leaf where the two documents disagree."""
    if found is None:
        found = []
    if len(found) > 8:
        return found

    if type(left) is not type(right):
        found.append((path or "/", f"<{type(left).__name__}>", f"<{type(right).__name__}>"))
    elif isinstance(left, dict):
        for key in sorted(set(left) | set(right)):
            if key not in left:
                found.append((f"{path}/{key}", "<absent>", json.dumps(right[key])[:80]))
            elif key not in right:
                found.append((f"{path}/{key}", json.dumps(left[key])[:80], "<absent>"))
            else:
                differences(left[key], right[key], f"{path}/{key}", found)
    elif isinstance(left, list):
        if len(left) != len(right):
            found.append((path or "/", f"{len(left)} items", f"{len(right)} items"))
        for index, (a, b) in enumerate(zip(left, right)):
            differences(a, b, f"{path}[{index}]", found)
    elif left != right:
        found.append((path or "/", json.dumps(left)[:80], json.dumps(right)[:80]))

    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--original", default="http://localhost:5043")
    parser.add_argument("--port", default="http://localhost:5055")
    parser.add_argument("--username", default="admin")
    parser.add_argument("--password", default="secret123")
    arguments = parser.parse_args()

    original_token = login(arguments.original, arguments.username, arguments.password)
    port_token = login(arguments.port, arguments.username, arguments.password)

    identical = []
    differing = []

    for path in ANONYMOUS + AUTHENTICATED:
        token = port_token if path in AUTHENTICATED else None
        original_status, original_body = fetch(arguments.original, path,
                                               original_token if token else None)
        port_status, port_body = fetch(arguments.port, path, token)

        if original_status == port_status and original_body == port_body:
            identical.append(path)
        else:
            differing.append((path, original_status, port_status, original_body, port_body))

    for path in identical:
        print(f"  same  {path}")

    for path, original_status, port_status, original_body, port_body in differing:
        print(f"  DIFF  {path}  ({original_status} vs {port_status})")
        for where, left, right in differences(original_body, port_body):
            print(f"          {where}\n            original = {left}\n            port     = {right}")

    total = len(identical) + len(differing)
    print(f"\n{len(identical)}/{total} routes identical")

    return 1 if differing else 0


if __name__ == "__main__":
    sys.exit(main())
