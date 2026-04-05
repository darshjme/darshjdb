#!/bin/bash
# Sets up the DarshanDB database for local development.
# Usage: ./scripts/setup-db.sh [DATABASE_URL]
#
# Examples:
#   ./scripts/setup-db.sh
#   DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshandb ./scripts/setup-db.sh
#   ./scripts/setup-db.sh postgres://darshan:darshan@localhost:5432/darshandb
#   ./scripts/setup-db.sh --seed   # run migration + seed data
#   ./scripts/setup-db.sh postgres://... --seed

set -euo pipefail

# ── Resolve database URL ────────────────────────────────────────────

SEED=false

for arg in "$@"; do
    case "$arg" in
        --seed) SEED=true ;;
        postgres://*) DATABASE_URL="$arg" ;;
    esac
done

DATABASE_URL="${DATABASE_URL:-postgres://darshan:darshan@localhost:5432/darshandb}"

# ── Locate migration files ──────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MIGRATIONS_DIR="$SCRIPT_DIR/../packages/server/migrations"

if [ ! -f "$MIGRATIONS_DIR/001_initial.sql" ]; then
    echo "ERROR: Migration file not found at $MIGRATIONS_DIR/001_initial.sql"
    exit 1
fi

# ── Wait for Postgres to be ready ───────────────────────────────────

echo "Waiting for Postgres..."
for i in $(seq 1 30); do
    if psql "$DATABASE_URL" -c "SELECT 1" > /dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Postgres not reachable at $DATABASE_URL after 30 seconds"
        exit 1
    fi
    sleep 1
done
echo "Postgres is ready."

# ── Run migration ───────────────────────────────────────────────────

echo "Running migration 001_initial.sql..."
psql "$DATABASE_URL" -f "$MIGRATIONS_DIR/001_initial.sql"
echo "Migration complete."

# ── Optionally seed ─────────────────────────────────────────────────

if [ "$SEED" = true ]; then
    if [ ! -f "$MIGRATIONS_DIR/seed.sql" ]; then
        echo "ERROR: Seed file not found at $MIGRATIONS_DIR/seed.sql"
        exit 1
    fi
    echo "Seeding database..."
    psql "$DATABASE_URL" -f "$MIGRATIONS_DIR/seed.sql"
    echo "Seed complete."
fi

# ── Report ──────────────────────────────────────────────────────────

TRIPLE_COUNT=$(psql "$DATABASE_URL" -t -A -c "SELECT count(*) FROM triples;" 2>/dev/null || echo "?")
echo ""
echo "=== DarshanDB Setup Complete ==="
echo "  Database:  $DATABASE_URL"
echo "  Triples:   $TRIPLE_COUNT"
echo ""
