# DarshJDB Python SDK

Official async Python client for [DarshJDB](https://db.darshj.me) — a real-time database with graph relations, live queries, auth, and storage.

## Installation

```bash
pip install darshjdb
```

Or with [uv](https://github.com/astral-sh/uv):

```bash
uv add darshjdb
```

## Quick Start

```python
from darshjdb import DarshDB

async def main():
    db = DarshDB("http://localhost:8080")
    await db.signin({"user": "root", "pass": "root"})
    await db.use("test", "test")

    # Create
    user = await db.create("users", {"name": "Darsh", "age": 30})

    # Read
    users = await db.select("users")
    user = await db.select("users:darsh")

    # Update
    await db.update("users:darsh", {"age": 31})

    # Delete
    await db.delete("users:darsh")

    # Query
    results = await db.query("SELECT * FROM users WHERE age > 18")

    # Graph relations
    await db.relate("user:darsh", "works_at", "company:knowai")

    # Live queries (WebSocket)
    async for change in db.live("SELECT * FROM users"):
        print(f"{change.action}: {change.result}")

    # Server-side functions
    report = await db.run("generateReport", {"month": "2026-04"})

    await db.close()
```

## API Reference

### Connection

```python
db = DarshDB("http://localhost:8080", timeout=30.0)

# Context manager (auto-close)
async with DarshDB("http://localhost:8080") as db:
    ...
```

### Authentication

```python
# Root/system auth
await db.signin({"user": "root", "pass": "root"})

# Email/password auth
await db.signin({"email": "alice@example.com", "password": "secret"})

# Signup
await db.signup({"email": "bob@example.com", "password": "pass", "name": "Bob"})

# Set token directly
await db.authenticate("existing-jwt-token")

# Sign out
await db.invalidate()
```

### Namespace & Database

```python
await db.use("namespace", "database")
```

### CRUD

```python
# Select all from table
users = await db.select("users")

# Select specific record
user = await db.select("users:darsh")

# Create
user = await db.create("users", {"name": "Darsh"})
user = await db.create("users:darsh", {"name": "Darsh"})  # with ID

# Insert (batch)
await db.insert("users", [{"name": "A"}, {"name": "B"}])

# Update (full replace)
await db.update("users:darsh", {"name": "Darsh", "age": 31})

# Merge (partial update)
await db.merge("users:darsh", {"age": 31})

# Delete
await db.delete("users:darsh")   # single record
await db.delete("users")          # all records in table
```

### Queries

```python
results = await db.query("SELECT * FROM users WHERE age > $min_age", vars={"min_age": 18})

for qr in results:
    print(qr.data)       # list of records
    print(qr.count)      # record count
    print(qr.duration_ms) # execution time
```

### Live Queries

```python
async for change in db.live("SELECT * FROM users"):
    print(change.action)  # LiveAction.CREATE / UPDATE / DELETE
    print(change.result)  # the record
```

### Graph Relations

```python
await db.relate("user:darsh", "works_at", "company:knowai")
await db.relate("user:darsh", "follows", "user:alice", {"since": "2026-01-01"})
```

### Server-Side Functions

```python
result = await db.run("generateReport", {"month": "2026-04"})
```

## Development

```bash
uv sync --dev
uv run pytest
uv run ruff check src/
```

## License

MIT
