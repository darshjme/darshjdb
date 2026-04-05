# darshan CLI

Command-line interface for DarshanDB -- development server, deployments, migrations, backups, and administration.

## Install

```bash
curl -fsSL https://darshandb.dev/install | sh
```

The CLI is a single Rust binary named `darshan`.

## Commands

### Development

| Command | Description |
|---------|-------------|
| `darshan dev` | Start dev server with hot reload, auto-creates Postgres if needed |
| `darshan dev --port 7701` | Start on a custom port |
| `darshan dev --database-url URL` | Use an external PostgreSQL instance |

### Production

| Command | Description |
|---------|-------------|
| `darshan start --prod` | Start the production server |
| `darshan deploy --functions` | Deploy updated server functions |

### Migrations

| Command | Description |
|---------|-------------|
| `darshan migrate generate --name NAME` | Generate a new migration from schema diff |
| `darshan migrate up` | Apply all pending migrations |
| `darshan migrate up --steps N` | Apply N migrations |
| `darshan migrate up --dry-run` | Show SQL without executing |
| `darshan migrate down --steps N` | Roll back N migrations |
| `darshan migrate down --all` | Roll back all migrations |
| `darshan migrate status` | Show migration status table |
| `darshan migrate resolve --applied NAME` | Mark a migration as applied without running it |

### Backups

| Command | Description |
|---------|-------------|
| `darshan backup --output PATH` | Create a compressed database backup |
| `darshan backup verify --input PATH` | Verify backup integrity |
| `darshan restore --input PATH` | Restore from a backup file |

### Administration

| Command | Description |
|---------|-------------|
| `darshan keys rotate` | Rotate encryption keys (re-encrypts all encrypted fields) |
| `darshan bench` | Run performance benchmarks |
| `darshan db ping` | Test PostgreSQL connectivity |
| `darshan reset --force` | Drop and recreate the database (development only) |
| `darshan debug-info` | Export diagnostic information |
| `darshan --version` | Show version |
| `darshan --help` | Show all commands |

### User Management

| Command | Description |
|---------|-------------|
| `darshan export --user EMAIL` | Export all data for a user (GDPR) |
| `darshan delete-user --email EMAIL` | Permanently delete a user and all their data |

## Examples

```bash
# Start development
darshan dev

# Generate and apply a migration
darshan migrate generate --name add-tags-to-todos
darshan migrate up

# Backup before upgrading
darshan backup --output /backups/darshan-pre-upgrade.sql.gz

# Benchmark your deployment
darshan bench --connections 100 --duration 30s --queries-per-sec 1000

# Check what's happening
darshan migrate status
darshan db ping
```

## Environment Variables

The CLI respects the same environment variables as the server:

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `DARSHAN_PORT` | Server port (default: 7700) |
| `DARSHAN_ADMIN_TOKEN` | Admin token for admin endpoints |
| `RUST_LOG` | Log level (trace, debug, info, warn, error) |

## Building from Source

```bash
# From the workspace root
cargo build --release -p darshan-cli

# Binary output
ls target/release/darshan
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
