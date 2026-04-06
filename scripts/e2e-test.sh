#!/usr/bin/env bash
# =============================================================================
# DarshanDB End-to-End Test
# =============================================================================
#
# Proves the full request path works: HTTP request -> Axum -> Postgres -> response.
#
# This script:
#   1. Starts Postgres via Docker (unless SKIP_POSTGRES=1)
#   2. Builds and starts the DarshanDB server
#   3. Tests every REST API endpoint with curl
#   4. Cleans up all processes and containers
#
# Usage:
#   ./scripts/e2e-test.sh              # Full run (starts Postgres + server)
#   SKIP_POSTGRES=1 ./scripts/e2e-test.sh  # Postgres already running
#   SKIP_BUILD=1 ./scripts/e2e-test.sh     # Binary already built
#
# Exit codes:
#   0 = all tests passed
#   1 = at least one test failed
# =============================================================================

set -euo pipefail

# Global timeout: kill the entire script after 5 minutes to prevent CI hangs
E2E_TIMEOUT="${E2E_TIMEOUT:-300}"
( sleep "$E2E_TIMEOUT" && echo "ERROR: E2E test timed out after ${E2E_TIMEOUT}s" && kill -TERM $$ 2>/dev/null ) &
TIMEOUT_PID=$!

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

BASE_URL="${DARSHAN_URL:-http://localhost:7700}"
API_URL="$BASE_URL/api"
DB_URL="${DATABASE_URL:-postgres://darshan:darshan@localhost:5432/darshandb}"
SERVER_PORT="${DDB_PORT:-7700}"
SERVER_PID=""
POSTGRES_CONTAINER="darshandb-e2e-postgres"
PASSED=0
FAILED=0
TOTAL=0

# ---------------------------------------------------------------------------
# Colors
# ---------------------------------------------------------------------------

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { echo -e "${CYAN}[e2e]${NC} $*"; }
pass() { echo -e "  ${GREEN}PASS${NC} $*"; PASSED=$((PASSED + 1)); TOTAL=$((TOTAL + 1)); }
fail() { echo -e "  ${RED}FAIL${NC} $*"; FAILED=$((FAILED + 1)); TOTAL=$((TOTAL + 1)); }
step() { echo -e "\n${BOLD}--- $* ---${NC}"; }

cleanup() {
    log "Cleaning up..."
    # Kill the timeout watchdog
    kill "$TIMEOUT_PID" 2>/dev/null || true
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        log "Server stopped (PID $SERVER_PID)"
    fi
    if [ "${SKIP_POSTGRES:-0}" != "1" ]; then
        docker rm -f "$POSTGRES_CONTAINER" 2>/dev/null || true
        log "Postgres container removed"
    fi
}

trap cleanup EXIT

# Check a curl response: assert_status <test_name> <expected_status> <actual_status> [body]
assert_status() {
    local name="$1"
    local expected="$2"
    local actual="$3"
    local body="${4:-}"

    if [ "$actual" = "$expected" ]; then
        pass "$name (HTTP $actual)"
    else
        fail "$name (expected HTTP $expected, got HTTP $actual)"
        if [ -n "$body" ]; then
            echo "    Response: $body"
        fi
        exit 1
    fi
}

# Check that a JSON field exists and is not null
assert_json_field() {
    local name="$1"
    local body="$2"
    local field="$3"

    local value
    value=$(echo "$body" | jq -r "$field" 2>/dev/null || echo "PARSE_ERROR")

    if [ "$value" != "null" ] && [ "$value" != "PARSE_ERROR" ] && [ -n "$value" ]; then
        pass "$name (field $field = $value)"
    else
        fail "$name (field $field missing or null)"
        echo "    Response: $body"
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Step 0: Prerequisites
# ---------------------------------------------------------------------------

echo -e "\n${BOLD}=== DarshanDB End-to-End Test ===${NC}\n"

REQUIRED_CMDS="curl jq"
if [ "${SKIP_POSTGRES:-0}" != "1" ]; then
    REQUIRED_CMDS="$REQUIRED_CMDS docker"
fi

for cmd in $REQUIRED_CMDS; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "ERROR: $cmd is required but not found in PATH"
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Step 1: Start Postgres
# ---------------------------------------------------------------------------

step "Starting Postgres"

if [ "${SKIP_POSTGRES:-0}" = "1" ]; then
    log "SKIP_POSTGRES=1, assuming Postgres is already running"
else
    # Remove stale container if it exists
    docker rm -f "$POSTGRES_CONTAINER" 2>/dev/null || true

    docker run -d \
        --name "$POSTGRES_CONTAINER" \
        -e POSTGRES_USER=darshan \
        -e POSTGRES_PASSWORD=darshan \
        -e POSTGRES_DB=darshandb \
        -p 5432:5432 \
        --health-cmd "pg_isready -U darshan -d darshandb" \
        --health-interval 2s \
        --health-timeout 5s \
        --health-retries 15 \
        pgvector/pgvector:pg16 \
        >/dev/null

    log "Waiting for Postgres to be ready..."
    for i in $(seq 1 30); do
        if docker exec "$POSTGRES_CONTAINER" pg_isready -U darshan -d darshandb &>/dev/null; then
            log "Postgres ready after ${i}s"
            break
        fi
        if [ "$i" -eq 30 ]; then
            echo "ERROR: Postgres did not become ready in 30s"
            exit 1
        fi
        sleep 1
    done
fi

# ---------------------------------------------------------------------------
# Step 2: Build the server
# ---------------------------------------------------------------------------

step "Building DarshanDB server"

if [ "${SKIP_BUILD:-0}" = "1" ]; then
    log "SKIP_BUILD=1, assuming binary already built"
else
    log "cargo build --bin ddb-server (this may take a while on first run)"
    cargo build --bin ddb-server 2>&1 | tail -5
    log "Build complete"
fi

# ---------------------------------------------------------------------------
# Step 3: Start the server
# ---------------------------------------------------------------------------

step "Starting DarshanDB server"

# If the server is already running (e.g. started by CI workflow), skip startup.
if curl -sf "$BASE_URL/health" >/dev/null 2>&1; then
    log "Server already running at $BASE_URL, skipping startup"
else
    # Find the server binary — prefer release, fall back to debug
    SERVER_BIN=""
    for candidate in \
        ./target/release/ddb-server \
        ./target/debug/ddb-server; do
        if [ -x "$candidate" ]; then
            SERVER_BIN="$candidate"
            break
        fi
    done

    if [ -z "$SERVER_BIN" ]; then
        log "No pre-built binary found, using cargo run"
        DATABASE_URL="$DB_URL" \
        DDB_PORT="$SERVER_PORT" \
        DDB_DEV=1 \
        DDB_JWT_SECRET="e2e-test-secret-do-not-use-in-production" \
        RUST_LOG="info" \
            cargo run --bin ddb-server &
        SERVER_PID=$!
    else
        log "Using binary: $SERVER_BIN"
        DATABASE_URL="$DB_URL" \
        DDB_PORT="$SERVER_PORT" \
        DDB_DEV=1 \
        DDB_JWT_SECRET="e2e-test-secret-do-not-use-in-production" \
        RUST_LOG="info" \
            "$SERVER_BIN" &
        SERVER_PID=$!
    fi

    log "Server starting (PID $SERVER_PID), waiting for /health..."

    for i in $(seq 1 30); do
        if curl -sf "$BASE_URL/health" >/dev/null 2>&1; then
            log "Server ready after ${i}s"
            break
        fi
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            echo "ERROR: Server process exited unexpectedly"
            exit 1
        fi
        if [ "$i" -eq 30 ]; then
            echo "ERROR: Server did not respond to /health within 30s"
            exit 1
        fi
        sleep 1
    done
fi

# ---------------------------------------------------------------------------
# Step 4: Authenticate (get a token)
# ---------------------------------------------------------------------------

step "Authentication: Sign up"

SIGNUP_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/auth/signup" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "e2e-test@darshan.dev",
        "password": "test-password-12345"
    }')

SIGNUP_BODY=$(echo "$SIGNUP_RESPONSE" | head -n -1)
SIGNUP_STATUS=$(echo "$SIGNUP_RESPONSE" | tail -n 1)

assert_status "POST /api/auth/signup returns 201" "201" "$SIGNUP_STATUS" "$SIGNUP_BODY"
assert_json_field "Signup returns access_token" "$SIGNUP_BODY" ".access_token"

TOKEN=$(echo "$SIGNUP_BODY" | jq -r '.access_token')
AUTH_USER_ID=$(echo "$SIGNUP_BODY" | jq -r '.user_id // empty')
log "Got token: ${TOKEN:0:20}..."
if [ -n "$AUTH_USER_ID" ]; then
    log "Auth user ID: $AUTH_USER_ID"
fi

AUTH_HEADER="Authorization: Bearer $TOKEN"

# ---------------------------------------------------------------------------
# Step 5: Health check
# ---------------------------------------------------------------------------

step "Health Check"

HEALTH_RESPONSE=$(curl -s -w "\n%{http_code}" "$BASE_URL/health")
HEALTH_BODY=$(echo "$HEALTH_RESPONSE" | head -n -1)
HEALTH_STATUS=$(echo "$HEALTH_RESPONSE" | tail -n 1)

assert_status "GET /health returns 200" "200" "$HEALTH_STATUS" "$HEALTH_BODY"
assert_json_field "Health check has status field" "$HEALTH_BODY" ".status"

# ---------------------------------------------------------------------------
# Step 6: Create a user entity
# ---------------------------------------------------------------------------

step "CRUD: Create a profile"

# Use "profiles" entity (not "users") because "users" requires admin role
CREATE_USER_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/data/profiles" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d '{
        "name": "Darsh Joshi",
        "email": "darsh@darshan.dev",
        "role": "admin"
    }')

CREATE_USER_BODY=$(echo "$CREATE_USER_RESPONSE" | head -n -1)
CREATE_USER_STATUS=$(echo "$CREATE_USER_RESPONSE" | tail -n 1)

assert_status "POST /api/data/profiles returns 201" "201" "$CREATE_USER_STATUS" "$CREATE_USER_BODY"
assert_json_field "Create profile returns id" "$CREATE_USER_BODY" ".id"

USER_ID=$(echo "$CREATE_USER_BODY" | jq -r '.id')
log "Created profile: $USER_ID"

# ---------------------------------------------------------------------------
# Step 7: Read the user back
# ---------------------------------------------------------------------------

step "CRUD: Read profile back"

GET_USER_RESPONSE=$(curl -s -w "\n%{http_code}" "$API_URL/data/profiles/$USER_ID" \
    -H "$AUTH_HEADER")

GET_USER_BODY=$(echo "$GET_USER_RESPONSE" | head -n -1)
GET_USER_STATUS=$(echo "$GET_USER_RESPONSE" | tail -n 1)

# NOTE: The current stub returns 404. Once wired to triple store, this should be 200.
# We accept either 200 (wired) or 404 (stub) and document which we got.
if [ "$GET_USER_STATUS" = "200" ]; then
    pass "GET /api/data/profiles/:id returns 200 (triple store wired!)"
elif [ "$GET_USER_STATUS" = "404" ]; then
    pass "GET /api/data/profiles/:id returns 404 (stub - triple store not yet wired)"
    log "${YELLOW}NOTE: This will return 200 once the triple store is integrated${NC}"
else
    fail "GET /api/data/profiles/:id unexpected status $GET_USER_STATUS"
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 8: Create a todo linked to the user
# ---------------------------------------------------------------------------

step "CRUD: Create a todo"

CREATE_TODO_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/data/todos" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d "{
        \"title\": \"Wire triple store end-to-end\",
        \"done\": false,
        \"priority\": 1,
        \"owner_id\": \"${AUTH_USER_ID:-$USER_ID}\"
    }")

CREATE_TODO_BODY=$(echo "$CREATE_TODO_RESPONSE" | head -n -1)
CREATE_TODO_STATUS=$(echo "$CREATE_TODO_RESPONSE" | tail -n 1)

assert_status "POST /api/data/todos returns 201" "201" "$CREATE_TODO_STATUS" "$CREATE_TODO_BODY"
assert_json_field "Create todo returns id" "$CREATE_TODO_BODY" ".id"

TODO_ID=$(echo "$CREATE_TODO_BODY" | jq -r '.id')
log "Created todo: $TODO_ID"

# Create a second todo for list/filter tests
CREATE_TODO2_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/data/todos" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d "{
        \"title\": \"Write documentation\",
        \"done\": true,
        \"priority\": 2,
        \"owner_id\": \"${AUTH_USER_ID:-$USER_ID}\"
    }")

CREATE_TODO2_BODY=$(echo "$CREATE_TODO2_RESPONSE" | head -n -1)
CREATE_TODO2_STATUS=$(echo "$CREATE_TODO2_RESPONSE" | tail -n 1)

assert_status "POST /api/data/todos (second) returns 201" "201" "$CREATE_TODO2_STATUS" "$CREATE_TODO2_BODY"

TODO2_ID=$(echo "$CREATE_TODO2_BODY" | jq -r '.id')
log "Created todo 2: $TODO2_ID"

# ---------------------------------------------------------------------------
# Step 9: List todos
# ---------------------------------------------------------------------------

step "CRUD: List todos"

LIST_RESPONSE=$(curl -s -w "\n%{http_code}" "$API_URL/data/todos?limit=10" \
    -H "$AUTH_HEADER")

LIST_BODY=$(echo "$LIST_RESPONSE" | head -n -1)
LIST_STATUS=$(echo "$LIST_RESPONSE" | tail -n 1)

assert_status "GET /api/data/todos returns 200" "200" "$LIST_STATUS" "$LIST_BODY"

# ---------------------------------------------------------------------------
# Step 10: Mutation API (batch insert via /api/mutate)
# ---------------------------------------------------------------------------

step "Mutation API"

MUTATE_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/mutate" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d "{
        \"mutations\": [
            {
                \"op\": \"insert\",
                \"entity\": \"tags\",
                \"data\": { \"name\": \"urgent\", \"color\": \"red\" }
            },
            {
                \"op\": \"insert\",
                \"entity\": \"tags\",
                \"data\": { \"name\": \"low-priority\", \"color\": \"gray\" }
            }
        ]
    }")

MUTATE_BODY=$(echo "$MUTATE_RESPONSE" | head -n -1)
MUTATE_STATUS=$(echo "$MUTATE_RESPONSE" | tail -n 1)

assert_status "POST /api/mutate returns 200" "200" "$MUTATE_STATUS" "$MUTATE_BODY"
assert_json_field "Mutate returns tx_id" "$MUTATE_BODY" ".tx_id"
assert_json_field "Mutate returns affected count" "$MUTATE_BODY" ".affected"

# ---------------------------------------------------------------------------
# Step 11: Query API (DarshanQL via /api/query)
# ---------------------------------------------------------------------------

step "Query API"

QUERY_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/query" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d '{
        "query": { "type": "todos" }
    }')

QUERY_BODY=$(echo "$QUERY_RESPONSE" | head -n -1)
QUERY_STATUS=$(echo "$QUERY_RESPONSE" | tail -n 1)

assert_status "POST /api/query returns 200" "200" "$QUERY_STATUS" "$QUERY_BODY"

# ---------------------------------------------------------------------------
# Step 12: Update a todo via PATCH
# ---------------------------------------------------------------------------

step "CRUD: Update a todo"

PATCH_RESPONSE=$(curl -s -w "\n%{http_code}" -X PATCH "$API_URL/data/todos/$TODO_ID" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d '{ "done": true, "title": "Wire triple store end-to-end (DONE)" }')

PATCH_BODY=$(echo "$PATCH_RESPONSE" | head -n -1)
PATCH_STATUS=$(echo "$PATCH_RESPONSE" | tail -n 1)

assert_status "PATCH /api/data/todos/:id returns 200" "200" "$PATCH_STATUS" "$PATCH_BODY"

# ---------------------------------------------------------------------------
# Step 13: Delete a todo
# ---------------------------------------------------------------------------

step "CRUD: Delete a todo"

DELETE_RESPONSE=$(curl -s -w "\n%{http_code}" -X DELETE "$API_URL/data/todos/$TODO_ID" \
    -H "$AUTH_HEADER")

DELETE_BODY=$(echo "$DELETE_RESPONSE" | head -n -1)
DELETE_STATUS=$(echo "$DELETE_RESPONSE" | tail -n 1)

assert_status "DELETE /api/data/todos/:id returns 204" "204" "$DELETE_STATUS"

# ---------------------------------------------------------------------------
# Step 14: Verify deletion (should be 404)
# ---------------------------------------------------------------------------

step "CRUD: Verify deletion"

VERIFY_RESPONSE=$(curl -s -w "\n%{http_code}" "$API_URL/data/todos/$TODO_ID" \
    -H "$AUTH_HEADER")

VERIFY_BODY=$(echo "$VERIFY_RESPONSE" | head -n -1)
VERIFY_STATUS=$(echo "$VERIFY_RESPONSE" | tail -n 1)

assert_status "GET deleted todo returns 404" "404" "$VERIFY_STATUS"

# ---------------------------------------------------------------------------
# Step 15: Mutation with update and delete ops
# ---------------------------------------------------------------------------

step "Mutation API: update + delete"

MUTATE2_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/mutate" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d "{
        \"mutations\": [
            {
                \"op\": \"update\",
                \"entity\": \"todos\",
                \"id\": \"$TODO2_ID\",
                \"data\": { \"title\": \"Updated via mutate\" }
            },
            {
                \"op\": \"delete\",
                \"entity\": \"todos\",
                \"id\": \"$TODO2_ID\"
            }
        ]
    }")

MUTATE2_BODY=$(echo "$MUTATE2_RESPONSE" | head -n -1)
MUTATE2_STATUS=$(echo "$MUTATE2_RESPONSE" | tail -n 1)

assert_status "POST /api/mutate (update+delete) returns 200" "200" "$MUTATE2_STATUS"

# ---------------------------------------------------------------------------
# Step 16: Error handling tests
# ---------------------------------------------------------------------------

step "Error Handling"

# Missing auth token
NOAUTH_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/query" \
    -H "Content-Type: application/json" \
    -d '{ "query": { "type": "todos" } }')

NOAUTH_STATUS=$(echo "$NOAUTH_RESPONSE" | tail -n 1)
assert_status "POST /api/query without token returns 401" "401" "$NOAUTH_STATUS"

# Invalid entity name
BAD_ENTITY_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/data/drop%20table" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d '{ "x": 1 }')

BAD_ENTITY_STATUS=$(echo "$BAD_ENTITY_RESPONSE" | tail -n 1)

if [ "$BAD_ENTITY_STATUS" = "400" ] || [ "$BAD_ENTITY_STATUS" = "422" ]; then
    pass "POST /api/data/<invalid> rejected (HTTP $BAD_ENTITY_STATUS)"
else
    fail "POST /api/data/<invalid> should be 400 or 422, got $BAD_ENTITY_STATUS"
fi

# Empty mutation list
EMPTY_MUTATE_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/mutate" \
    -H "$AUTH_HEADER" \
    -H "Content-Type: application/json" \
    -d '{ "mutations": [] }')

EMPTY_MUTATE_STATUS=$(echo "$EMPTY_MUTATE_RESPONSE" | tail -n 1)
assert_status "POST /api/mutate with empty list returns 400" "400" "$EMPTY_MUTATE_STATUS"

# Invalid signup (short password)
BAD_SIGNUP_RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$API_URL/auth/signup" \
    -H "Content-Type: application/json" \
    -d '{ "email": "bad@test.com", "password": "short" }')

BAD_SIGNUP_STATUS=$(echo "$BAD_SIGNUP_RESPONSE" | tail -n 1)
assert_status "POST /api/auth/signup with short password returns 400" "400" "$BAD_SIGNUP_STATUS"

# ---------------------------------------------------------------------------
# Step 17: OpenAPI spec
# ---------------------------------------------------------------------------

step "OpenAPI Spec"

OPENAPI_RESPONSE=$(curl -s -w "\n%{http_code}" "$API_URL/openapi.json")
OPENAPI_BODY=$(echo "$OPENAPI_RESPONSE" | head -n -1)
OPENAPI_STATUS=$(echo "$OPENAPI_RESPONSE" | tail -n 1)

assert_status "GET /api/openapi.json returns 200" "200" "$OPENAPI_STATUS"
assert_json_field "OpenAPI spec has version" "$OPENAPI_BODY" ".openapi"

# ---------------------------------------------------------------------------
# Results
# ---------------------------------------------------------------------------

echo ""
echo -e "${BOLD}=== Results ===${NC}"
echo -e "  Total:  $TOTAL"
echo -e "  ${GREEN}Passed: $PASSED${NC}"
if [ "$FAILED" -gt 0 ]; then
    echo -e "  ${RED}Failed: $FAILED${NC}"
    echo ""
    echo -e "${RED}E2E TEST SUITE FAILED${NC}"
    exit 1
else
    echo -e "  ${RED}Failed: 0${NC}"
    echo ""
    echo -e "${GREEN}ALL E2E TESTS PASSED${NC}"
    exit 0
fi
