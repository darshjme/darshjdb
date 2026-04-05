# DarshJDB cURL Scripts

Shell scripts that exercise the DarshJDB REST API using cURL. Useful for testing, debugging, and understanding the API surface without writing application code.

## Prerequisites

- A running DarshJDB server (default: `http://localhost:7700`)
- `curl` and `jq` installed
- Bash shell

## Scripts

### `auth.sh` -- Authentication

Signs up a demo user, signs in, exports the token, and fetches the current user profile.

```bash
# Source it to export DDB_TOKEN to your shell
source examples/curl-scripts/auth.sh

# Use custom credentials
source examples/curl-scripts/auth.sh user@example.com mypassword
```

### `crud.sh` -- CRUD Operations

Creates, reads, updates, lists, and deletes a todo. Requires `DDB_TOKEN` from `auth.sh`.

```bash
source examples/curl-scripts/auth.sh
bash examples/curl-scripts/crud.sh
```

### `functions.sh` -- Server Functions

Invokes server-side query and mutation functions, checks health, and introspects the schema.

```bash
source examples/curl-scripts/auth.sh
bash examples/curl-scripts/functions.sh
```

## Configuration

Set `DDB_URL` to point at a different server:

```bash
export DDB_URL=https://db.myapp.com
source examples/curl-scripts/auth.sh
```
