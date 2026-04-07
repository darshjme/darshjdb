#!/usr/bin/env bash
# ── DarshJDB LXC Container Provisioning ──────────────────────────────
# Creates and provisions an LXC container running DarshJDB + PostgreSQL.
#
# Usage (run on the LXC host):
#   sudo bash setup.sh [container-name] [ddb-version]
#
# Prerequisites:
#   - LXC/LXD installed on the host
#   - DarshJDB release binary available at:
#     https://github.com/darshjme/darshjdb/releases/download/v${VERSION}/ddb-server-linux-amd64
#
# What this script does:
#   1. Creates a Debian 12 (bookworm) LXC container
#   2. Installs PostgreSQL 16, Redis 7, system deps
#   3. Downloads the DarshJDB release binary
#   4. Creates a systemd service for ddb-server
#   5. Applies the initial migration
#   6. Starts all services
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

CONTAINER_NAME="${1:-darshjdb}"
DDB_VERSION="${2:-0.1.0}"
DDB_PORT="${DDB_PORT:-7700}"
PG_PASSWORD="${PG_PASSWORD:-$(openssl rand -base64 24)}"
JWT_SECRET="${JWT_SECRET:-$(openssl rand -base64 32)}"
RELEASE_BASE="https://github.com/darshjme/darshjdb/releases/download"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log()  { echo -e "${GREEN}[darshjdb]${NC} $*"; }
warn() { echo -e "${YELLOW}[darshjdb]${NC} $*"; }
die()  { echo -e "${RED}[darshjdb]${NC} $*" >&2; exit 1; }

# ── Preflight ────────────────────────────────────────────────────────
command -v lxc >/dev/null 2>&1 || die "lxc command not found. Install LXC/LXD first."
[[ $EUID -eq 0 ]] || die "Run this script as root (sudo)."

# ── Create container ─────────────────────────────────────────────────
log "Creating LXC container '${CONTAINER_NAME}' (Debian 12)..."
if lxc info "${CONTAINER_NAME}" &>/dev/null; then
    warn "Container '${CONTAINER_NAME}' already exists. Skipping creation."
else
    lxc launch images:debian/12 "${CONTAINER_NAME}" \
        -c limits.cpu=4 \
        -c limits.memory=2GB \
        -c security.nesting=false
    # Wait for network
    log "Waiting for container network..."
    for i in $(seq 1 30); do
        if lxc exec "${CONTAINER_NAME}" -- ping -c1 -W1 8.8.8.8 &>/dev/null; then
            break
        fi
        sleep 1
    done
fi

# ── Provision inside the container ───────────────────────────────────
log "Provisioning container..."

lxc exec "${CONTAINER_NAME}" -- bash -euo pipefail <<PROVISION
export DEBIAN_FRONTEND=noninteractive

# ── System packages ──────────────────────────────────────────────────
apt-get update -qq
apt-get install -y --no-install-recommends \
    ca-certificates curl gnupg lsb-release \
    libssl3 tini

# ── PostgreSQL 16 repo + install ─────────────────────────────────────
curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc | \
    gpg --dearmor -o /usr/share/keyrings/pgdg.gpg
echo "deb [signed-by=/usr/share/keyrings/pgdg.gpg] http://apt.postgresql.org/pub/repos/apt bookworm-pgdg main" \
    > /etc/apt/sources.list.d/pgdg.list
apt-get update -qq
apt-get install -y --no-install-recommends postgresql-16 postgresql-16-pgvector

# ── Redis 7 ──────────────────────────────────────────────────────────
apt-get install -y --no-install-recommends redis-server

# ── Clean up apt cache ───────────────────────────────────────────────
apt-get clean && rm -rf /var/lib/apt/lists/*

# ── Configure PostgreSQL ─────────────────────────────────────────────
sudo -u postgres psql <<SQL
CREATE USER darshan WITH PASSWORD '${PG_PASSWORD}';
CREATE DATABASE darshjdb OWNER darshan;
\\c darshjdb
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
SQL

# ── Configure Redis ──────────────────────────────────────────────────
sed -i 's/^# maxmemory .*/maxmemory 128mb/' /etc/redis/redis.conf
sed -i 's/^# maxmemory-policy .*/maxmemory-policy allkeys-lru/' /etc/redis/redis.conf
systemctl restart redis-server

# ── Create darshjdb user ─────────────────────────────────────────────
useradd -r -s /sbin/nologin -d /opt/darshjdb -m darshan || true
mkdir -p /opt/darshjdb/{bin,data,admin}
chown -R darshan:darshan /opt/darshjdb

# ── Download DarshJDB binary ─────────────────────────────────────────
BINARY_URL="${RELEASE_BASE}/v${DDB_VERSION}/ddb-server-linux-amd64"
echo "Downloading ddb-server v${DDB_VERSION}..."
curl -fsSL -o /opt/darshjdb/bin/ddb-server "\${BINARY_URL}" || {
    echo "WARNING: Could not download release binary."
    echo "Place the binary manually at /opt/darshjdb/bin/ddb-server"
}
chmod 555 /opt/darshjdb/bin/ddb-server 2>/dev/null || true

# ── Environment file (secrets live here, not in systemd unit) ────────
cat > /opt/darshjdb/.env <<ENV
DATABASE_URL=postgres://darshan:${PG_PASSWORD}@127.0.0.1:5432/darshjdb
DDB_PORT=${DDB_PORT}
DDB_ADMIN_DIR=/opt/darshjdb/admin
DDB_DATA_DIR=/opt/darshjdb/data
DDB_JWT_SECRET=${JWT_SECRET}
DDB_REDIS_URL=redis://127.0.0.1:6379/0
RUST_LOG=info
ENV
chmod 600 /opt/darshjdb/.env
chown darshan:darshan /opt/darshjdb/.env

# ── Systemd service ─────────────────────────────────────────────────
cat > /etc/systemd/system/darshjdb.service <<SVC
[Unit]
Description=DarshJDB Server
Documentation=https://db.darshj.me
After=network.target postgresql.service redis-server.service
Requires=postgresql.service

[Service]
Type=simple
User=darshan
Group=darshan
EnvironmentFile=/opt/darshjdb/.env
ExecStart=/opt/darshjdb/bin/ddb-server
Restart=on-failure
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=5

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/opt/darshjdb/data
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
MemoryMax=512M

[Install]
WantedBy=multi-user.target
SVC

systemctl daemon-reload
systemctl enable darshjdb.service

# ── Start services ───────────────────────────────────────────────────
systemctl start postgresql
systemctl start redis-server
systemctl start darshjdb || echo "WARNING: ddb-server binary may not be present yet."

echo ""
echo "========================================="
echo " DarshJDB LXC provisioning complete"
echo "========================================="
echo " Container:  ${CONTAINER_NAME}"
echo " DDB Port:   ${DDB_PORT}"
echo " PG Password: ${PG_PASSWORD}"
echo " JWT Secret:  (stored in /opt/darshjdb/.env)"
echo ""
echo " Save these credentials securely."
echo "========================================="
PROVISION

# ── Proxy port from host to container ────────────────────────────────
log "Adding proxy device for port ${DDB_PORT}..."
lxc config device add "${CONTAINER_NAME}" ddb-port proxy \
    listen=tcp:0.0.0.0:${DDB_PORT} \
    connect=tcp:127.0.0.1:${DDB_PORT} 2>/dev/null || \
    warn "Proxy device may already exist."

log "Done. Access DarshJDB at http://<host-ip>:${DDB_PORT}"
log "Container shell: lxc exec ${CONTAINER_NAME} -- bash"
