# DarshanDB Python SDK

Official Python SDK for [DarshanDB](https://github.com/darshandb/darshandb) with sync and async support.

## Requirements

- Python 3.9+
- httpx 0.24+

## Installation

```bash
pip install darshandb
```

For real-time SSE subscriptions:

```bash
pip install darshandb[sse]
```

## Quick Start

```python
from darshandb import DarshanDB

db = DarshanDB(server_url="https://db.example.com", api_key="your-key")

# Auth
db.auth.sign_up("alice@example.com", "password123", display_name="Alice")
db.auth.sign_in("alice@example.com", "password123")
user = db.auth.get_user()
db.auth.sign_out()

# Query
result = db.query({
    "collection": "posts",
    "where": [{"field": "published", "op": "=", "value": True}],
    "order": [{"field": "createdAt", "direction": "desc"}],
    "limit": 20,
})

# Convenience helpers
posts = db.get("posts", where=[{"field": "published", "op": "=", "value": True}], limit=20)
post = db.create("posts", {"title": "Hello", "body": "My first post."})
db.update("posts", post["id"], {"title": "Updated"})
db.delete("posts", post["id"])

# Transactions
db.transact([
    {"kind": "set", "entity": "accounts", "id": "a1", "data": {"balance": 900}},
    {"kind": "set", "entity": "accounts", "id": "a2", "data": {"balance": 1100}},
])

# Server-side functions
report = db.fn("generateReport", {"month": "2026-04"})

# Storage
result = db.storage.upload("/avatars/photo.jpg", "/tmp/photo.jpg")
url = db.storage.get_url("/avatars/photo.jpg")
db.storage.delete("/avatars/photo.jpg")

db.close()
```

## Context Manager

```python
with DarshanDB(server_url="https://db.example.com", api_key="key") as db:
    db.auth.sign_in("user@example.com", "password")
    posts = db.get("posts", limit=10)
# HTTP client is automatically closed
```

## Admin Client

```python
from darshandb import DarshanAdmin

admin = DarshanAdmin(
    server_url="https://db.example.com",
    admin_token="dsk_admin_...",
)

# Impersonate a user
user_db = admin.as_user("alice@example.com")
posts = user_db.query({"collection": "posts"})

# Admin-level queries (bypass permissions)
all_users = admin.query({"collection": "users", "limit": 1000})

# Real-time subscriptions (async, requires darshandb[sse])
import asyncio

async def watch_orders():
    async for event in admin.subscribe({"collection": "orders"}):
        print(f"Order update: {event}")

asyncio.run(watch_orders())
```

## FastAPI Integration

```python
from contextlib import asynccontextmanager
from fastapi import FastAPI, Depends, HTTPException
from darshandb import DarshanDB

db: DarshanDB | None = None

@asynccontextmanager
async def lifespan(app: FastAPI):
    global db
    db = DarshanDB(
        server_url="https://db.example.com",
        api_key="your-key",
    )
    yield
    db.close()

app = FastAPI(lifespan=lifespan)

def get_db() -> DarshanDB:
    assert db is not None
    return db

@app.get("/posts")
def list_posts(darshan: DarshanDB = Depends(get_db)):
    result = darshan.get(
        "posts",
        where=[{"field": "published", "op": "=", "value": True}],
        order=[{"field": "createdAt", "direction": "desc"}],
        limit=20,
    )
    return result["data"]

@app.post("/posts")
def create_post(title: str, body: str, darshan: DarshanDB = Depends(get_db)):
    post = darshan.create("posts", {"title": title, "body": body, "published": False})
    return post

@app.post("/auth/signin")
def sign_in(email: str, password: str, darshan: DarshanDB = Depends(get_db)):
    try:
        result = darshan.auth.sign_in(email, password)
        return {"token": result["accessToken"], "user": result["user"]}
    except Exception:
        raise HTTPException(status_code=401, detail="Invalid credentials")
```

## Django Integration

```python
# settings.py
DARSHAN_SERVER_URL = "https://db.example.com"
DARSHAN_API_KEY = "your-key"

# darshan_client.py
from django.conf import settings
from darshandb import DarshanDB

_client: DarshanDB | None = None

def get_darshan() -> DarshanDB:
    global _client
    if _client is None:
        _client = DarshanDB(
            server_url=settings.DARSHAN_SERVER_URL,
            api_key=settings.DARSHAN_API_KEY,
        )
    return _client

# views.py
from django.http import JsonResponse
from django.views import View
from .darshan_client import get_darshan

class PostListView(View):
    def get(self, request):
        db = get_darshan()
        result = db.get(
            "posts",
            where=[{"field": "published", "op": "=", "value": True}],
            order=[{"field": "createdAt", "direction": "desc"}],
            limit=20,
        )
        return JsonResponse({"posts": result["data"]})

    def post(self, request):
        import json
        data = json.loads(request.body)
        db = get_darshan()
        post = db.create("posts", {
            "title": data["title"],
            "body": data["body"],
        })
        return JsonResponse(post, status=201)

# middleware.py
from .darshan_client import get_darshan

class DarshanAuthMiddleware:
    def __init__(self, get_response):
        self.get_response = get_response

    def __call__(self, request):
        token = request.headers.get("Authorization", "").removeprefix("Bearer ").strip()
        if token:
            db = get_darshan()
            db.auth.set_token(token)
            request.darshan_db = db
        return self.get_response(request)
```

## Error Handling

```python
from darshandb import DarshanDB, DarshanAPIError

db = DarshanDB(server_url="https://db.example.com", api_key="key")

try:
    db.auth.sign_in("user@example.com", "wrong-password")
except DarshanAPIError as e:
    print(e)                 # "invalid credentials"
    print(e.status_code)     # 401
    print(e.error_body)      # {"error": "invalid credentials", ...}
```

## Configuration

| Parameter    | Type  | Default | Description                      |
| ------------ | ----- | ------- | -------------------------------- |
| `server_url` | str   | --      | DarshanDB server URL (required)  |
| `api_key`    | str   | --      | Application API key (required)   |
| `timeout`    | float | 30.0    | HTTP timeout in seconds          |

## License

MIT
