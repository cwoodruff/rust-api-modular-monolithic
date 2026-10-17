# The original's Dockerfile cannot produce a working container, in three
# separate ways: it builds net10.0 projects with the .NET 9 SDK, it copies the
# whole working tree without a .dockerignore, and the image it would produce
# runs as Production — where the development signing-key provider throws at
# startup, so the host exits before it listens. This one works.

# --- build ----------------------------------------------------------------
FROM rust:1-slim-bookworm AS build

WORKDIR /src

# Manifests first, so a source-only change does not re-download the registry.
COPY Cargo.toml Cargo.lock ./
COPY crates/api/Cargo.toml crates/api/
COPY crates/architecture-tests/Cargo.toml crates/architecture-tests/
COPY crates/administration-contracts/Cargo.toml crates/administration-contracts/
COPY crates/catalog-contracts/Cargo.toml crates/catalog-contracts/
COPY crates/identity-contracts/Cargo.toml crates/identity-contracts/
COPY crates/orders-contracts/Cargo.toml crates/orders-contracts/
COPY crates/module-admin/Cargo.toml crates/module-admin/
COPY crates/module-catalog/Cargo.toml crates/module-catalog/
COPY crates/module-identity/Cargo.toml crates/module-identity/
COPY crates/module-orders/Cargo.toml crates/module-orders/
COPY crates/module-reporting/Cargo.toml crates/module-reporting/
COPY crates/module-tests/Cargo.toml crates/module-tests/
COPY crates/shared-kernel/Cargo.toml crates/shared-kernel/
COPY crates/test-support/Cargo.toml crates/test-support/
COPY crates/validation-rules/Cargo.toml crates/validation-rules/
COPY samples/catalog-host/Cargo.toml samples/catalog-host/

# Stub sources so the dependency graph resolves and builds on its own layer.
# Each module embeds its migrations at compile time, so their directories
# must exist too.
RUN set -eux; \
    for crate in api architecture-tests administration-contracts catalog-contracts identity-contracts orders-contracts module-admin module-catalog module-identity module-orders module-reporting module-tests shared-kernel test-support validation-rules; do \
        mkdir -p "crates/$crate/src" "crates/$crate/migrations"; \
        echo "" > "crates/$crate/src/lib.rs"; \
    done; \
    mkdir -p samples/catalog-host/src; \
    echo "fn main() {}" > samples/catalog-host/src/main.rs; \
    echo "fn main() {}" > crates/api/src/main.rs

RUN cargo build --release --locked -p api

# Now the real sources. Removing the stubs' fingerprints forces a rebuild of
# the workspace crates while leaving the dependency layer cached.
COPY crates crates
COPY samples samples
RUN set -eux; \
    find target/release -maxdepth 2 -name '*modular_monolith_api*' -delete; \
    find target/release/.fingerprint -maxdepth 1 \
        \( -name 'api-*' -o -name 'module-*' -o -name 'shared-*' -o -name '*-contracts-*' -o -name 'validation-rules-*' \) -exec rm -rf {} +; \
    cargo build --release --locked -p api

# --- runtime --------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# curl is here for the healthcheck; ca-certificates for any outbound TLS a
# future key provider needs.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    rm -rf /var/lib/apt/lists/*; \
    useradd --system --create-home --uid 10001 api

WORKDIR /app

COPY --from=build /src/target/release/modular-monolith-api /usr/local/bin/
COPY appsettings.json appsettings.Development.json appsettings.Demo.json ./
# The seed is loaded only where the host migrates on startup — Development,
# Test and Demo. Production expects a database already migrated and seeded.
COPY data/chinook-postgres-seed.sql data/chinook-postgres-seed.sql

# The development signing key is written here on first run, so the directory
# has to be writable by the unprivileged user.
RUN set -eux; \
    mkdir -p /app/data/identity; \
    chown -R api:api /app

USER api

# The port the original's Dockerfile exposes.
EXPOSE 8080
ENV Port=8080

# There is no database in the image: point ConnectionStrings__AppDatabase and
# ConnectionStrings__Reporting at a PostgreSQL 17 server.
#
# Deliberately not defaulted to Development or Demo. Those unlock Swagger, the
# in-memory login store and the development signing key, and an image that
# quietly enables all three is the wrong default. Left unset the host runs as
# Production and exits at startup with a message naming what to configure —
# which is the original's behavior too, just without the mystery.
#
#   A runnable demo, against the compose database on the host:
#     docker compose up -d
#     docker run -p 8080:8080 -e ASPNETCORE_ENVIRONMENT=Demo \
#       -e ConnectionStrings__AppDatabase="Host=host.docker.internal;Port=5432;Database=chinook;Username=chinook;Password=chinook" \
#       -e ConnectionStrings__Reporting="Host=host.docker.internal;Port=5432;Database=chinook;Username=reporting;Password=reporting" \
#       modular-monolith-api
#
#   Production needs a real key provider:
#     -e Jwt__KeyProvider=KeyVault -e Jwt__KeyVaultVaultUri=... -e Jwt__KeyVaultKeyName=...

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl --fail --silent --output /dev/null http://localhost:8080/ || exit 1

ENTRYPOINT ["modular-monolith-api"]
