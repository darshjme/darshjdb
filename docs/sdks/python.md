# Python SDK

The `darshjdb` Python package is a fully async client for DarshJDB, built on `httpx` for HTTP and `websockets` for live queries. It follows SurrealDB's SDK ergonomics: sign in, select a namespace/database, then CRUD, query, live-subscribe, and manage graph relations.

## Installation

```bash
pip install darshjdb
```

Dependencies: `httpx`, `websockets` (for live queries only).

## Quick Start

```python
from darshjdb import DarshDB

async with DarshDB("http://localhost:8080") as db:
    await db.signin({"user": "root", "pass": "root"})
    await db.use("test", "test")

    user = await db.create("users", {"name": "Darsh", "age": 30})
    users = await db.select("users")
    print(users)
```

## Connection

```python
db = DarshDB("http://localhost:8080", timeout=30.0)

# Check health
is_healthy = await db.health()  # True / False
version = await db.version()    # e.g. "0.1.0"

# Always close when done
await db.close()
```

The client can also be used as an async context manager, which ensures cleanup:

```python
async with DarshDB("http://localhost:8080") as db:
    # db.close() is called automatically on exit
    ...
```

### Connection State

```python
from darshjdb.models import ConnectionState

db.state  # ConnectionState.CONNECTED / DISCONNECTED / CLOSING
```

## Authentication

### Root/System Auth

```python
auth = await db.signin({"user": "root", "pass": "root"})
print(auth.token)
print(auth.user)
```

### User Auth (Email/Password)

```python
# Sign up
auth = await db.signup({
    "email": "alice@example.com",
    "password": "s3cret",
    "name": "Alice",
})

# Sign in
auth = await db.signin({
    "email": "alice@example.com",
    "password": "s3cret",
})

# Sign out
await db.invalidate()
```

### Restore Session

```python
await db.authenticate("eyJhbGciOiJIUzI1NiIs...")
```

### Scoped Auth

```python
auth = await db.signin({
    "email": "alice@example.com",
    "password": "s3cret",
    "namespace": "production",
    "database": "main",
})
```

The `AuthResponse` object contains `token`, `user` (dict), and `refresh_token`.

## Namespace and Database

```python
await db.use("production", "main")
```

Sets headers `X-DarshDB-NS` and `X-DarshDB-DB` on all subsequent requests.

## CRUD Operations

### Select

```python
# All records in a table
users = await db.select("users")

# Specific record by ID
user = await db.select("users:darsh")  # Returns a single-element list
```

### Create

```python
user = await db.create("users", {"name": "Darsh", "age": 30})
# Returns the created record with server-assigned ID

# With explicit ID
user = await db.create("users:darsh", {"name": "Darsh", "age": 30})
```

### Insert (Batch)

```python
results = await db.insert("users", [
    {"name": "Alice", "age": 25},
    {"name": "Bob", "age": 32},
])
```

### Update (Full Replace)

```python
updated = await db.update("users:darsh", {"name": "Darsh", "age": 31})
```

Requires a record ID (e.g., `"users:darsh"`). Replaces all fields.

### Merge (Partial Update)

```python
merged = await db.merge("users:darsh", {"age": 31})
```

Only updates the specified fields; other fields are preserved.

### Delete

```python
# Single record
await db.delete("users:darsh")

# All records in a table
await db.delete("users")
```

## Queries

### DarshJQL Queries

```python
results = await db.query("SELECT * FROM users WHERE age > 18")

for result in results:
    print(result.data)  # list of records
    print(result.meta)  # query metadata
```

### Parameterized Queries

```python
results = await db.query(
    "SELECT * FROM users WHERE age > $min_age AND name = $name",
    vars={"min_age": 18, "name": "Alice"},
)
```

### Raw Queries

For direct access to the unparsed server response:

```python
raw = await db.query_raw("SELECT * FROM users")
```

## Live Queries

Subscribe to real-time changes via WebSocket. `live()` returns an async iterator that yields `LiveNotification` objects.

```python
from darshjdb.models import LiveAction

async for change in db.live("SELECT * FROM users"):
    print(f"Action: {change.action}")  # LiveAction.CREATE / UPDATE / DELETE
    print(f"Data: {change.result}")

# Or subscribe to an entire table:
async for change in db.live("users"):
    if change.action == LiveAction.CREATE:
        print("New user:", change.result)
```

### Diff Mode

Receive diff patches instead of full records:

```python
async for change in db.live("users", diff=True):
    print(change.result)
```

### SSE Subscriptions

For Server-Sent Events instead of WebSocket:

```python
channel = await db.subscribe("users", callback=my_handler)

async def my_handler(event: dict):
    print("SSE event:", event)
```

## Graph Relations

Create and manage edges between records:

```python
await db.relate("user:darsh", "works_at", "company:knowai")

# With edge data
await db.relate(
    "user:darsh",
    "works_at",
    "company:knowai",
    data={"role": "CTO", "since": "2024-01-01"},
)
```

## Server Functions

Invoke registered server-side functions:

```python
result = await db.run("generateReport", {"month": 3, "year": 2026})
```

## Batch Operations

Execute multiple operations in a single HTTP request:

```python
results = await db.batch([
    {"method": "GET", "path": "/api/data/users"},
    {"method": "POST", "path": "/api/data/posts", "body": {"title": "Hello"}},
])
```

## File Storage

### Upload

```python
result = await db.upload(
    path="/avatars/photo.jpg",
    content=file_bytes,
    filename="photo.jpg",
    content_type="image/jpeg",
)
print(result["url"])
```

### Download

```python
content = await db.download("/avatars/photo.jpg")
with open("photo.jpg", "wb") as f:
    f.write(content)
```

## Error Handling

The SDK raises typed exceptions:

| Exception               | When                                        |
|--------------------------|---------------------------------------------|
| `DarshDBConnectionError` | Network unreachable / connection refused     |
| `DarshDBAuthError`       | Invalid credentials (401/403)                |
| `DarshDBQueryError`      | Query parse or execution error               |
| `DarshDBAPIError`        | Any other 4xx/5xx server response            |
| `DarshDBError`           | Base class for all DarshJDB errors           |

```python
from darshjdb.exceptions import DarshDBAuthError, DarshDBQueryError

try:
    await db.signin({"email": "x@y.com", "password": "wrong"})
except DarshDBAuthError as e:
    print("Auth failed:", e)

try:
    await db.query("INVALID SQL")
except DarshDBQueryError as e:
    print("Query failed:", e)
    print("Query was:", e.query)
```

## FastAPI Integration

```python
from contextlib import asynccontextmanager
from fastapi import FastAPI, Depends
from darshjdb import DarshDB

db: DarshDB | None = None

@asynccontextmanager
async def lifespan(app: FastAPI):
    global db
    db = DarshDB("http://localhost:8080")
    await db.signin({"user": "root", "pass": "root"})
    await db.use("production", "main")
    yield
    await db.close()

app = FastAPI(lifespan=lifespan)

def get_db() -> DarshDB:
    assert db is not None
    return db

@app.get("/users")
async def list_users(db: DarshDB = Depends(get_db)):
    return await db.select("users")
```

## Django Integration

```python
# myapp/db.py
from darshjdb import DarshDB

_db: DarshDB | None = None

async def get_db() -> DarshDB:
    global _db
    if _db is None:
        _db = DarshDB("http://localhost:8080")
        await _db.signin({"user": "root", "pass": "root"})
        await _db.use("production", "main")
    return _db

# myapp/views.py
from django.http import JsonResponse
from myapp.db import get_db

async def user_list(request):
    db = await get_db()
    users = await db.select("users")
    return JsonResponse({"users": users})
```
