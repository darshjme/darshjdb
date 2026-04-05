# darshan CLI

Command-line interface for DarshanDB -- development server, deployments, migrations, and administration.

## Install

```bash
curl -fsSL https://darshandb.dev/install | sh
```

The CLI is a single Rust binary named `darshan`.

## Commands

| Command | Description |
|---------|-------------|
| `darshan dev` | Start the development server with hot reload |
| `darshan start --prod` | Start the production server |
| `darshan migrate generate` | Generate a new migration file |
| `darshan migrate up` | Apply pending migrations |
| `darshan migrate down` | Roll back migrations |
| `darshan migrate status` | Show migration status |
| `darshan backup` | Create a database backup |
| `darshan restore` | Restore from a backup |
| `darshan bench` | Run performance benchmarks |
| `darshan keys rotate` | Rotate encryption keys |
| `darshan --version` | Show version |

## Development

```bash
# From the workspace root
cargo build -p darshan-cli

# Run directly
cargo run -p darshan-cli -- dev
```

## Building

```bash
cargo build --release -p darshan-cli
```

The binary is output to `target/release/darshan`.

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Self-Hosting](../../docs/self-hosting.md)
- [Migration Guide](../../docs/migration.md)
