# ── Stage 1: Build the admin dashboard ───────────────────────────────
# Slice 19/30: the dashboard is embedded into ddb-server at compile time
# via include_dir!, so it MUST be built before the rust crate.
FROM node:22-alpine AS frontend

WORKDIR /build

COPY package.json package-lock.json ./
COPY packages/client-core/package.json packages/client-core/package.json
COPY packages/react/package.json packages/react/package.json
COPY packages/admin/package.json packages/admin/package.json

RUN mkdir -p packages/client-core packages/react packages/admin && \
    npm ci --workspace=packages/client-core \
           --workspace=packages/react \
           --workspace=packages/admin

COPY packages/client-core/ packages/client-core/
COPY packages/react/ packages/react/
COPY packages/admin/ packages/admin/

RUN npm run build --workspace=packages/admin

# ── Stage 2: Build the Rust server and CLI ───────────────────────────
FROM rust:1.92-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config libssl-dev musl-tools && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Build locally or on the deployment server; GitHub Actions is disabled.
# BuildKit caches registry downloads and compiled dependencies between runs.

COPY Cargo.toml Cargo.lock ./
COPY packages/server/        packages/server/
COPY packages/cli/           packages/cli/
COPY packages/cache/         packages/cache/
COPY packages/cache-server/  packages/cache-server/
COPY packages/agent-memory/  packages/agent-memory/
COPY --from=frontend /build/packages/admin/dist packages/admin/dist

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --workspace && \
    strip target/release/ddb-server target/release/ddb target/release/ddb-cache-server

# ── Stage 3: Runtime (minimal) ───────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install only what we need: TLS certs, tini as PID 1, curl for healthcheck
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates libssl3 tini curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd -r darshan && useradd -r -g darshan -d /app -s /sbin/nologin darshan

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /build/target/release/ddb-server       /usr/local/bin/ddb-server
COPY --from=builder /build/target/release/ddb              /usr/local/bin/ddb
COPY --from=builder /build/target/release/ddb-cache-server /usr/local/bin/ddb-cache-server
COPY --from=frontend /build/packages/admin/dist            /usr/share/darshan/admin

# Lock down binary permissions
RUN chmod 555 /usr/local/bin/ddb-server /usr/local/bin/ddb /usr/local/bin/ddb-cache-server && \
    mkdir -p /app/data && \
    chown -R darshan:darshan /app

USER darshan

# Runtime configuration — all overridable via environment
ENV DDB_ADMIN_DIR=/usr/share/darshan/admin \
    DDB_PORT=7700 \
    DDB_DATA_DIR=/app/data \
    RUST_LOG=info

EXPOSE 7700

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -sf http://127.0.0.1:7700/health || exit 1

ENTRYPOINT ["tini", "--"]
CMD ["ddb-server"]

# ── Metadata ─────────────────────────────────────────────────────────
LABEL org.opencontainers.image.title="DarshJDB" \
      org.opencontainers.image.description="Backend-as-a-Service with triple store, DarshJQL, and real-time sync" \
      org.opencontainers.image.url="https://db.darshj.me" \
      org.opencontainers.image.source="https://github.com/darshjme/darshjdb" \
      org.opencontainers.image.vendor="DarshJ" \
      org.opencontainers.image.licenses="MIT"
