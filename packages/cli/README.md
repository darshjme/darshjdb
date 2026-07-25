# ddb CLI

Command-line interface for DarshJDB -- development server, SQL shell, data import/export, and deployment.

## Install

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
cargo build --release
# Binary at ./target/release/ddb
```

The CLI is a single Rust binary named `ddb`.

## Commands

### Server

| Command | Description |
|---------|-------------|
| `ddb start` | Start the server (PostgreSQL-backed) |
| `ddb start --conn URL --bind 0.0.0.0:7700` | Use an explicit database and bind address |
| `ddb start --user EMAIL --pass SECRET` | Create/update the root admin user on startup |
| `ddb start --strict` | Enforce `schema_definitions` on writes |
| `ddb dev` | Start with debug logging and function hot reload |
| `ddb dev --port 7701 --watch false` | Custom port, hot reload disabled |

`--storage memory` is not an in-process store: DarshJDB is PostgreSQL-backed, so
memory mode requires `--conn` (or `DDB_MEMORY_URL`) pointing at a disposable database.

### Data

| Command | Description |
|---------|-------------|
| `ddb sql --conn URL` | Interactive DarshQL shell |
| `ddb sql --user EMAIL --pass SECRET` | Shell authenticated via `/api/auth/signin` |
| `ddb export --output PATH --format json\|jsonl` | Export all entities |
| `ddb import FILE` | Import a `.json` export or `.jsonl` stream |
| `ddb seed FILE` | Run a seed file against the database |

### Project & Deployment

| Command | Description |
|---------|-------------|
| `ddb init [NAME]` | Scaffold `ddb.toml` and the `darshan/` project tree |
| `ddb push --dir darshan/functions` | Push local functions to the server |
| `ddb pull --output darshan/generated` | Generate TypeScript types from the server schema |
| `ddb deploy --tag TAG --registry REG` | Build and push a Docker image |
| `ddb status` | Server health (`/health/full`) |
| `ddb upgrade` | Self-update from GitHub releases |
| `ddb --version` / `ddb --help` | Version and full command list |

## Examples

```bash
# Start development with hot reload
ddb dev

# Scaffold a project
ddb init my-app

# Explore data
ddb sql --conn http://localhost:7700

# Move data between instances
ddb export --output dump.json
ddb import dump.json --yes

# Check what's happening
ddb status
```

## Environment Variables

The CLI respects the same environment variables as the server:

| Variable | Description |
|----------|-------------|
| `DDB_URL` | Server URL for remote commands (default: `http://localhost:7700`) |
| `DDB_TOKEN` | Bearer token for remote commands |
| `DDB_MEMORY_URL` | Disposable database used by `--storage memory` |
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
