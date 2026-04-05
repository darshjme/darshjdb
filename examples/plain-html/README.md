# DarshJDB Plain HTML Example

A zero-dependency todo app using DarshJDB's REST API with vanilla JavaScript. No build tools, no frameworks -- just a single HTML file.

## What it demonstrates

- **Authentication** (sign up + sign in) via REST
- **CRUD operations** using `fetch()` against the DarshJDB API
- **Polling for updates** (2-second interval)
- How DarshJDB works without any SDK -- pure HTTP

## Prerequisites

- A running DarshJDB server (default: `http://localhost:7700`)
- Any modern web browser

## Setup

Open `index.html` directly in your browser, or serve it with any static file server:

```bash
# Using Python
cd examples/plain-html
python3 -m http.server 8080

# Using Node.js
npx serve examples/plain-html

# Or just open the file
open examples/plain-html/index.html
```

## How it works

The app authenticates with a demo account on load, then uses the DarshJDB REST API directly:

| Operation | Method | Endpoint |
|-----------|--------|----------|
| Sign up | `POST` | `/api/auth/signup` |
| Sign in | `POST` | `/api/auth/signin` |
| Query | `POST` | `/api/query` |
| Create | `POST` | `/api/data/{collection}` |
| Update | `PATCH` | `/api/data/{collection}/{id}` |
| Delete | `DELETE` | `/api/data/{collection}/{id}` |

All requests after auth include an `Authorization: Bearer <token>` header.
