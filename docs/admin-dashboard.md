# Admin Dashboard

The DarshanDB admin dashboard is a built-in web interface for managing your database, users, functions, storage, and server configuration. It is accessible at `http://localhost:7700/admin` when the server is running.

## Overview

The dashboard is a React + Vite + Tailwind CSS single-page application bundled with the server binary. It communicates with DarshanDB via the same REST and WebSocket APIs available to your application.

### Authentication

The admin dashboard requires an admin token to access. On first run, `darshan dev` generates an admin token and prints it to the console:

```
DarshanDB dev server running on http://localhost:7700
Admin dashboard: http://localhost:7700/admin
Admin token: drshn_admin_abc123...
```

In production, set the token via environment variable:

```bash
DARSHAN_ADMIN_TOKEN=your-secure-admin-token
```

## Pages

### Data Explorer

**Route:** `/`

The data explorer lets you browse all entities in your database, run DarshanQL queries interactively, and view results in a tabular format.

**Features:**
- Entity browser with type-ahead search
- DarshanQL query editor with syntax highlighting (CodeMirror)
- Sortable, paginated results table with type-aware cell rendering
- Click any entity ID to see its full attribute map
- Click any reference ID to navigate to the linked entity
- Export query results as JSON or CSV

**Query Editor:**

The embedded query editor supports the full DarshanQL syntax. Press `Cmd+Enter` (or `Ctrl+Enter`) to execute.

```
{ todos: { $where: { done: false }, $order: { createdAt: "desc" }, $limit: 50, owner: {} } }
```

### Schema

**Route:** `/schema`

Visualize your database schema -- entity types, their attributes, and relationships.

**Features:**
- List of all entity types with record counts
- Per-entity attribute table: name, type, cardinality, example values
- Relationship graph showing links between entity types
- Index information (which attributes are indexed)
- Strict mode status indicator

### Functions

**Route:** `/functions`

Monitor and manage server functions.

**Features:**
- Function registry: list all registered queries, mutations, actions, scheduled jobs, and internal functions
- Execution history with latency, status (success/error), and arguments
- Execution chart: visualize function call frequency and latency over time
- Click any function to see its source code, arguments schema, and recent invocations
- Manual invocation: call any function with custom arguments from the dashboard

### Auth / Users

**Route:** `/auth`

User management and session monitoring.

**Features:**
- User list with search, filter by role, sort by creation date
- User detail view: email, role, custom claims, MFA status, created/last seen dates
- Session list per user with device info and IP addresses
- Impersonation: view data as any user to test permission rules
- Create/edit/delete users
- Reset passwords
- Revoke sessions

### Storage

**Route:** `/storage`

File browser for the storage backend.

**Features:**
- Grid and list view toggle
- Directory navigation with breadcrumb trail
- File preview (images, PDFs, text files)
- File metadata: size, MIME type, upload date, custom metadata
- Drag-and-drop upload
- Generate signed URLs
- Delete files

### Logs

**Route:** `/logs`

Real-time log viewer.

**Features:**
- Live streaming of server logs over WebSocket
- Filter by log level: trace, debug, info, warn, error
- Filter by component: query, mutation, auth, permissions, sync, storage, functions
- Search within logs
- Pause/resume streaming
- Export log window as text

### Settings

**Route:** `/settings`

Server configuration and administration.

**Features:**
- Environment variable viewer (secrets are masked)
- Backup management: trigger manual backup, view backup history
- Rate limit configuration
- Webhook configuration for external integrations
- Server info: version, uptime, PostgreSQL version, connection pool stats

## Command Palette

Press `Cmd+K` (or `Ctrl+K`) anywhere in the dashboard to open the command palette. It provides fuzzy search across:

- All pages and navigation items
- Entity types (jump to Data Explorer filtered by entity)
- Functions (jump to function detail)
- Users (jump to user detail)
- Quick actions: "Create backup", "Clear cache", "Restart functions"

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+K` | Open command palette |
| `Cmd+Enter` | Execute query (in Data Explorer) |
| `Escape` | Close modal / command palette |
| `Cmd+Shift+D` | Toggle dark/light mode |

## Embedding in Production

The admin dashboard is compiled to static files during the Docker build. In production, DarshanDB serves these files from `DARSHAN_ADMIN_DIR`.

To disable the admin dashboard in production (security hardening):

```bash
DARSHAN_ADMIN_ENABLED=false
```

To restrict admin dashboard access by IP:

```bash
DARSHAN_ADMIN_ALLOWED_IPS=10.0.0.0/8,192.168.1.0/24
```

## Development

To develop the admin dashboard itself:

```bash
cd packages/admin
npm install
npm run dev
# Dashboard runs on http://localhost:3100, proxies /api to http://localhost:7700
```

See the [packages/admin README](../packages/admin/README.md) for full development instructions.

---

[Previous: Migration Guide](migration.md) | [Next: Troubleshooting](troubleshooting.md) | [All Docs](README.md)
