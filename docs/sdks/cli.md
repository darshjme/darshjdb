# CLI Reference

`ddb` is a single binary that serves as both the DarshJDB server and administration CLI. One binary to start, query, export, import, deploy, and manage.

## Installation

```bash
# From source
cargo install darshjdb

# Or download the binary
curl -fsSL https://get.darshj.me/ddb | sh
```

## Global Flags

These flags apply to all subcommands:

| Flag         | Env Variable  | Description                         |
|--------------|---------------|-------------------------------------|
| `--url`      | `DDB_URL`     | DarshJDB server URL                 |
| `--token`    | `DDB_TOKEN`   | Authentication token                |

## Commands

### ddb start

Launch a full DarshJDB server instance with HTTP API, WebSocket sync, triple store, auth engine, and all subsystems.

```bash
ddb start --bind 0.0.0.0:7700 --user root --pass root
```

| Flag           | Default              | Description                                      |
|----------------|----------------------|--------------------------------------------------|
| `--storage`    | `postgres`           | Storage backend (`postgres`)                     |
| `--conn`       | (auto)               | Connection string (e.g. `postgres://user:pass@host:port/db`) |
| `--bind`, `-b` | `0.0.0.0:7700`       | Address and port to bind                         |
| `--user`, `-u` | --                   | Initial root username                            |
| `--pass`, `-p` | --                   | Initial root password                            |
| `--log`        | `info`               | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `--strict`     | `false`              | Reject unknown fields, enforce schemas           |
| `--no-banner`  | `false`              | Suppress the startup banner                      |

### ddb dev

Start a local development server with dev-friendly defaults (debug logging, auto-reload).

```bash
ddb dev --port 7700
```

| Flag           | Default | Description                              |
|----------------|---------|------------------------------------------|
| `--port`, `-p` | `7700`  | Port to listen on                        |
| `--watch`      | `true`  | Watch for file changes and hot-reload    |

This is an alias for `ddb start` with `--log debug` and memory-friendly defaults.

### ddb sql

Open an interactive DarshQL shell with syntax highlighting, table output, and query history.

```bash
ddb sql --conn http://localhost:7700 --user root --pass root
```

| Flag           | Default                   | Description              |
|----------------|---------------------------|--------------------------|
| `--conn`, `-c` | `http://localhost:7700`   | Server connection URL    |
| `--user`, `-u` | --                        | Username                 |
| `--pass`, `-p` | --                        | Password                 |
| `--ns`         | --                        | Namespace to use         |
| `--db`         | --                        | Database to use          |
| `--pretty`     | `true`                    | Format output as tables  |

Example session:

```
ddb> SELECT * FROM users WHERE age > 18;
+---------+--------+-----+
| id      | name   | age |
+---------+--------+-----+
| users:1 | Alice  |  25 |
| users:2 | Bob    |  30 |
+---------+--------+-----+
2 rows (3.2ms)
```

### ddb init

Initialize a new DarshJDB project in the current directory. Creates the `darshan/` directory structure with default configuration.

```bash
ddb init my-project
```

| Argument | Default | Description    |
|----------|---------|----------------|
| `name`   | `.`     | Project name   |

### ddb deploy

Build and deploy a Docker image to production.

```bash
ddb deploy --tag v1.2.0 --registry ghcr.io/myorg/myapp
```

| Flag                | Default    | Description                          |
|---------------------|------------|--------------------------------------|
| `--tag`, `-t`       | `latest`   | Docker image tag                     |
| `--registry`, `-r`  | --         | Docker registry URL                  |
| `--yes`             | `false`    | Skip confirmation prompt             |

### ddb push

Push local server-side functions to the running server.

```bash
ddb push --dir darshan/functions
```

| Flag            | Default               | Description                               |
|-----------------|----------------------|-------------------------------------------|
| `--dir`, `-d`   | `darshan/functions`  | Functions directory                       |
| `--dry-run`     | `false`              | Show what would be pushed without pushing |

### ddb pull

Pull the current schema from the server and generate TypeScript types.

```bash
ddb pull --output darshan/generated
```

| Flag              | Default              | Description                    |
|-------------------|----------------------|--------------------------------|
| `--output`, `-o`  | `darshan/generated`  | Output directory for types     |

### ddb seed

Run a seed file against the database to populate initial data.

```bash
ddb seed darshan/seed.ts
```

| Argument | Default           | Description                      |
|----------|-------------------|----------------------------------|
| `file`   | `darshan/seed.ts` | Seed file path (TypeScript/JSON) |

### ddb migrate

Run database migrations.

```bash
# Apply pending migrations
ddb migrate

# Rollback last 2 migrations
ddb migrate --rollback 2

# Check migration status
ddb migrate --status
```

| Flag             | Default              | Description                          |
|------------------|----------------------|--------------------------------------|
| `--dir`, `-d`    | `darshan/migrations` | Migrations directory                 |
| `--rollback`     | --                   | Roll back the last N migrations      |
| `--status`       | `false`              | Show status without running          |

### ddb logs

Tail structured logs from a running server.

```bash
ddb logs --follow --level error
```

| Flag             | Default | Description                             |
|------------------|---------|-----------------------------------------|
| `-n`, `--lines`  | `100`   | Number of recent lines                  |
| `-f`, `--follow` | `false` | Follow log output (like `tail -f`)      |
| `-l`, `--level`  | --      | Filter: `debug`, `info`, `warn`, `error`|

### ddb auth

Authentication and user management subcommands.

#### ddb auth create-admin

```bash
ddb auth create-admin --email admin@example.com --password s3cret
```

| Flag             | Description                              |
|------------------|------------------------------------------|
| `--email`, `-e`  | Admin email address                      |
| `--password`, `-p` | Password (prompted interactively if omitted) |

#### ddb auth list-users

```bash
ddb auth list-users --limit 50
```

| Flag             | Default | Description               |
|------------------|---------|---------------------------|
| `--limit`, `-l`  | `50`    | Maximum users to display  |

#### ddb auth revoke-user

Revoke all active sessions for a user.

```bash
ddb auth revoke-user user@example.com
```

### ddb backup

Create a database backup.

```bash
ddb backup --output backup-2026-04-07.ddb --include-storage
```

| Flag               | Default | Description                          |
|--------------------|---------|--------------------------------------|
| `--output`, `-o`   | (auto)  | Output file path                     |
| `--include-storage` | `false` | Include file storage blobs           |

### ddb restore

Restore a database from a backup file.

```bash
ddb restore backup-2026-04-07.ddb --yes
```

| Flag    | Default | Description              |
|---------|---------|--------------------------|
| `--yes` | `false` | Skip confirmation prompt |

### ddb export

Export all data from a DarshJDB instance.

```bash
ddb export --conn http://localhost:7700 --output data.json --format json
```

| Flag             | Default                 | Description        |
|------------------|-------------------------|--------------------|
| `--conn`, `-c`   | `http://localhost:7700` | Server URL         |
| `--output`, `-o` | stdout                  | Output file path   |
| `--format`       | `json`                  | Export format      |

### ddb import

Import data into a DarshJDB instance.

```bash
ddb import data.json --conn http://localhost:7700 --yes
```

| Flag    | Default | Description              |
|---------|---------|--------------------------|
| `--yes` | `false` | Skip confirmation prompt |

### ddb status

Show server health, connection status, and system information.

```bash
ddb status
```

### ddb version

Display DarshJDB version, build hash, architecture, OS, and Rust compiler version.

```bash
ddb version
```

### ddb upgrade

Upgrade the DarshJDB binary to the latest version.

```bash
ddb upgrade --version 0.2.0 --yes
```

| Flag        | Default  | Description               |
|-------------|----------|---------------------------|
| `--version` | latest   | Target version            |
| `--yes`     | `false`  | Skip confirmation prompt  |

## Configuration File

The CLI reads configuration from the following locations (in order of precedence):

1. Command-line flags
2. Environment variables (`DDB_URL`, `DDB_TOKEN`)
3. Project config file (`darshan/config.toml` or `.ddb.toml`)

```toml
# darshan/config.toml
url = "http://localhost:7700"
token = "your-admin-token"
```
