# ── Stage 1: Build the Rust server and CLI ───────────────────────────
FROM rust:1.88-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconf

WORKDIR /build

# Cache dependency layer
COPY Cargo.toml Cargo.lock ./
COPY packages/server/Cargo.toml packages/server/Cargo.toml
COPY packages/cli/Cargo.toml packages/cli/Cargo.toml

RUN mkdir -p packages/server/src packages/cli/src && \
    echo "fn main() {}" > packages/server/src/main.rs && \
    echo "fn main() {}" > packages/cli/src/main.rs && \
    cargo build --release --workspace 2>/dev/null || true

# Copy real source
COPY packages/server/ packages/server/
COPY packages/cli/ packages/cli/

RUN touch packages/server/src/main.rs packages/cli/src/main.rs && \
    cargo build --release --workspace

# ── Stage 2: Build the admin dashboard ───────────────────────────────
FROM node:22-alpine AS frontend

WORKDIR /build

# Copy lockfile and root package.json first for cache
COPY package.json package-lock.json ./
COPY packages/client-core/package.json packages/client-core/package.json
COPY packages/react/package.json packages/react/package.json
COPY packages/admin/package.json packages/admin/package.json

# Create workspace dirs so npm ci can resolve them
RUN mkdir -p packages/client-core packages/react packages/admin

# Use npm ci for deterministic installs; scope to needed workspaces
RUN npm ci --workspace=packages/client-core --workspace=packages/react --workspace=packages/admin

# Now copy the actual source
COPY packages/client-core/ packages/client-core/
COPY packages/react/ packages/react/
COPY packages/admin/ packages/admin/

RUN npm run build --workspace=packages/admin

# ── Stage 3: Runtime ─────────────────────────────────────────────────
FROM alpine:3.21 AS runtime

RUN apk add --no-cache ca-certificates tini && \
    addgroup -S darshan && adduser -S darshan -G darshan

WORKDIR /app

COPY --from=builder /build/target/release/darshandb-server /usr/local/bin/darshandb-server
COPY --from=builder /build/target/release/darshan /usr/local/bin/darshan
COPY --from=frontend /build/packages/admin/dist /usr/share/darshan/admin

# Ensure binaries are not writable by runtime user
RUN chmod 555 /usr/local/bin/darshandb-server /usr/local/bin/ddb && \
    chown -R darshan:darshan /app

USER darshan

ENV DARSHAN_ADMIN_DIR=/usr/share/darshan/admin
ENV DARSHAN_PORT=7700
ENV RUST_LOG=info

EXPOSE 7700

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:7700/api/admin/health || exit 1

ENTRYPOINT ["tini", "--"]
CMD ["darshandb-server"]
