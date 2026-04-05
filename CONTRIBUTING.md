# Contributing to DarshJDB

We welcome contributions from everyone. Whether you're fixing a typo, reporting a bug, or building a new SDK — you're making DarshJDB better for every developer who uses it.

## Getting Started

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb

# Start the dev server (requires Docker for Postgres)
ddb dev

# Or manually:
docker compose up postgres -d
cargo run --bin ddb-server
```

## Project Structure

| Directory | Language | What it does |
|-----------|----------|-------------|
| `packages/server/` | Rust | Core server: triple store, query engine, sync, auth, functions |
| `packages/cli/` | Rust | CLI tool: `ddb dev`, `ddb deploy`, etc. |
| `packages/client-core/` | TypeScript | Framework-agnostic client SDK |
| `packages/react/` | TypeScript | React hooks SDK |
| `packages/angular/` | TypeScript | Angular signals/RxJS SDK |
| `packages/nextjs/` | TypeScript | Next.js App Router/Pages Router SDK |
| `packages/admin/` | TypeScript | Admin dashboard (React + Vite) |
| `sdks/php/` | PHP | PHP + Laravel SDK |
| `sdks/python/` | Python | Python + FastAPI/Django SDK |

## Development

### Rust

```bash
cargo fmt          # Format
cargo clippy       # Lint
cargo test         # Test
```

### TypeScript

```bash
npm install
npm run lint       # Lint all packages
npm test           # Test all packages
```

### PHP

```bash
cd sdks/php
composer install
vendor/bin/phpunit
```

### Python

```bash
cd sdks/python
pip install -e ".[dev]"
pytest
```

## Pull Request Guidelines

1. **One PR, one concern.** Don't mix features with refactors.
2. **Conventional commits.** `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
3. **Tests required.** Every change needs tests. No exceptions.
4. **No file over 400 lines.** Split it if it grows.
5. **No `unwrap()` in Rust.** Use `?` operator or explicit error handling.
6. **No `any` in TypeScript.** Use proper types or add a comment explaining why.

## Code of Conduct

Be respectful. Be constructive. We're all here to build something good.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
