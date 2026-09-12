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
COPY crates/module-admin/Cargo.toml crates/module-admin/
COPY crates/module-identity/Cargo.toml crates/module-identity/
COPY crates/module-music/Cargo.toml crates/module-music/
COPY crates/module-orders/Cargo.toml crates/module-orders/
COPY crates/module-reporting/Cargo.toml crates/module-reporting/
COPY crates/shared-data-sqlite/Cargo.toml crates/shared-data-sqlite/
COPY crates/shared-kernel/Cargo.toml crates/shared-kernel/
COPY crates/shared-persistence/Cargo.toml crates/shared-persistence/

# Stub sources so the dependency graph resolves and builds on its own layer.
RUN set -eux; \
    for crate in api architecture-tests module-admin module-identity module-music \
                 module-orders module-reporting shared-data-sqlite shared-kernel \
                 shared-persistence; do \
        mkdir -p "crates/$crate/src"; \
        echo "" > "crates/$crate/src/lib.rs"; \
    done; \
    mkdir -p crates/api/src; \
    echo "fn main() {}" > crates/api/src/main.rs

RUN cargo build --release --locked -p api

# Now the real sources. Removing the stubs' fingerprints forces a rebuild of
# the workspace crates while leaving the dependency layer cached.
COPY crates crates
RUN set -eux; \
    find target/release -maxdepth 2 -name '*modular_monolith_api*' -delete; \
    find target/release/.fingerprint -maxdepth 1 \
        \( -name 'api-*' -o -name 'module-*' -o -name 'shared-*' \) -exec rm -rf {} +; \
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
COPY appsettings.json appsettings.Development.json ./
COPY data/chinook.db data/chinook.db

# The development signing key is written here on first run, so the directory
# has to be writable by the unprivileged user.
RUN set -eux; \
    mkdir -p /app/data/identity; \
    chown -R api:api /app

USER api

# The port the original's Dockerfile exposes.
EXPOSE 8080
ENV Port=8080

# Deliberately not defaulted to Development or Demo. Those unlock Swagger, the
# in-memory login store and the development signing key, and an image that
# quietly enables all three is the wrong default. Left unset the host runs as
# Production and exits at startup with a message naming what to configure —
# which is the original's behavior too, just without the mystery.
#
#   A runnable demo:
#     docker run -p 8080:8080 -e ASPNETCORE_ENVIRONMENT=Demo modular-monolith-api
#
#   Production needs a real key provider:
#     -e Jwt__KeyProvider=KeyVault -e Jwt__KeyVaultVaultUri=... -e Jwt__KeyVaultKeyName=...

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl --fail --silent --output /dev/null http://localhost:8080/ || exit 1

ENTRYPOINT ["modular-monolith-api"]
