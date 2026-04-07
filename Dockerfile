# ── Stage 1: Build the Rust server and CLI ───────────────────────────
FROM rust:1.87-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config libssl-dev musl-tools && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependency layer — copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY packages/server/Cargo.toml packages/server/Cargo.toml
COPY packages/cli/Cargo.toml packages/cli/Cargo.toml

# Create stub sources so cargo can resolve the dependency graph
RUN mkdir -p packages/server/src packages/cli/src && \
    echo "fn main() {}" > packages/server/src/main.rs && \
    echo "fn main() {}" > packages/cli/src/main.rs && \
    cargo build --release --workspace 2>/dev/null || true && \
    rm -rf packages/server/src packages/cli/src

# Copy real source and rebuild (only changed crates recompile)
COPY packages/server/ packages/server/
COPY packages/cli/ packages/cli/

RUN touch packages/server/src/main.rs packages/cli/src/main.rs && \
    cargo build --release --workspace && \
    strip target/release/ddb-server target/release/ddb

# ── Stage 2: Build the admin dashboard ───────────────────────────────
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
COPY --from=builder /build/target/release/ddb-server /usr/local/bin/ddb-server
COPY --from=builder /build/target/release/ddb         /usr/local/bin/ddb
COPY --from=frontend /build/packages/admin/dist       /usr/share/darshan/admin

# Lock down binary permissions
RUN chmod 555 /usr/local/bin/ddb-server /usr/local/bin/ddb && \
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
    CMD curl -sf http://localhost:7700/health || exit 1

ENTRYPOINT ["tini", "--"]
CMD ["ddb-server"]

# ── Metadata ─────────────────────────────────────────────────────────
LABEL org.opencontainers.image.title="DarshJDB" \
      org.opencontainers.image.description="Backend-as-a-Service with triple store, DarshJQL, and real-time sync" \
      org.opencontainers.image.url="https://db.darshj.me" \
      org.opencontainers.image.source="https://github.com/darshjme/darshjdb" \
      org.opencontainers.image.vendor="DarshJ" \
      org.opencontainers.image.licenses="MIT"
