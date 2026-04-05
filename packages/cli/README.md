# ddb CLI

Command-line interface for DarshJDB -- development server, deployments, migrations, backups, and administration.

## Install

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo build --release
# Binary at ./target/release/ddb
```

The CLI is a single Rust binary named `ddb`.

## Commands

### Development

| Command | Description |
|---------|-------------|
| `ddb dev` | Start dev server with hot reload, auto-creates Postgres if needed |
| `ddb dev --port 7701` | Start on a custom port |
| `ddb dev --database-url URL` | Use an external PostgreSQL instance |

### Production

| Command | Description |
|---------|-------------|
| `ddb start --prod` | Start the production server |
| `ddb deploy --functions` | Deploy updated server functions |

### Migrations

| Command | Description |
|---------|-------------|
| `ddb migrate generate --name NAME` | Generate a new migration from schema diff |
| `ddb migrate up` | Apply all pending migrations |
| `ddb migrate up --steps N` | Apply N migrations |
| `ddb migrate up --dry-run` | Show SQL without executing |
| `ddb migrate down --steps N` | Roll back N migrations |
| `ddb migrate down --all` | Roll back all migrations |
| `ddb migrate status` | Show migration status table |
| `ddb migrate resolve --applied NAME` | Mark a migration as applied without running it |

### Backups

| Command | Description |
|---------|-------------|
| `ddb backup --output PATH` | Create a compressed database backup |
| `ddb backup verify --input PATH` | Verify backup integrity |
| `ddb restore --input PATH` | Restore from a backup file |

### Administration

| Command | Description |
|---------|-------------|
| `ddb keys rotate` | Rotate encryption keys (re-encrypts all encrypted fields) |
| `ddb bench` | Run performance benchmarks |
| `ddb db ping` | Test PostgreSQL connectivity |
| `ddb reset --force` | Drop and recreate the database (development only) |
| `ddb debug-info` | Export diagnostic information |
| `ddb --version` | Show version |
| `ddb --help` | Show all commands |

### User Management

| Command | Description |
|---------|-------------|
| `ddb export --user EMAIL` | Export all data for a user (GDPR) |
| `ddb delete-user --email EMAIL` | Permanently delete a user and all their data |

## Examples

```bash
# Start development
ddb dev

# Generate and apply a migration
ddb migrate generate --name add-tags-to-todos
ddb migrate up

# Backup before upgrading
ddb backup --output /backups/ddb-pre-upgrade.sql.gz

# Benchmark your deployment
ddb bench --connections 100 --duration 30s --queries-per-sec 1000

# Check what's happening
ddb migrate status
ddb db ping
```

## Environment Variables

The CLI respects the same environment variables as the server:

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `DDB_PORT` | Server port (default: 7700) |
| `DDB_ADMIN_TOKEN` | Admin token for admin endpoints |
| `RUST_LOG` | Log level (trace, debug, info, warn, error) |

## Building from Source

```bash
# From the workspace root
cargo build --release -p ddb-cli

# Binary output
ls target/release/ddb
```

## Key Dependencies

- **clap** -- CLI argument parsing with derive macros
- **tokio** -- Async runtime
- **reqwest** -- HTTP client for deploy and remote commands
- **colored** + **indicatif** -- Terminal colors and progress bars
- **dialoguer** -- Interactive prompts

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Self-Hosting](../../docs/self-hosting.md)
- [Migration Guide](../../docs/migration.md)
- [Troubleshooting](../../docs/troubleshooting.md)
