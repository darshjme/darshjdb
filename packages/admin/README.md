# @darshjdb/admin

Admin dashboard for DarshJDB -- data explorer, schema viewer, function registry, user management, storage browser, and real-time log viewer.

## Stack

- **React 18** with TypeScript
- **Vite 6** for dev and production builds
- **Tailwind CSS 3** for styling (dark-first design)
- **Radix UI** for accessible primitives (dialog, dropdown, select, switch, tabs, tooltip, popover)
- **Recharts** for data visualizations
- **CodeMirror 6** for the query editor with SQL syntax highlighting
- **Lucide React** for icons
- **React Router 6** for client-side routing

## Pages

| Route | Component | Description |
|-------------|-----------------|----------------------------------------------|
| `/` | DataExplorer | Browse entities, run DarshanQL queries, export results |
| `/schema` | Schema | View entity types, attributes, relationships, and indexes |
| `/functions` | Functions | Function registry, execution chart, invocation history |
| `/auth` | AuthUsers | User management, sessions, permissions, impersonation |
| `/storage` | Storage | File browser with grid/list view, drag-drop upload, previews |
| `/logs` | Logs | Real-time log viewer with level filtering and search |
| `/settings` | Settings | Env vars, backups, rate limits, webhooks |

## Components

| Component | Description |
|-----------|-------------|
| **Sidebar** | Collapsible navigation with active route highlighting |
| **TopBar** | Title, connection status indicator, command palette trigger, notifications |
| **CommandPalette** | `Cmd+K` fuzzy search across all pages, entities, functions, and users |
| **DataTable** | Generic sortable, paginated table with type-aware cell rendering |
| **Modal** | Accessible dialog with Escape-to-close and backdrop click |
| **Badge** | Color-coded status/label indicator (7 variants) |

## Development

```bash
# From monorepo root
npm install

# Start dev server (port 3100, proxies /api to localhost:7700)
npm run dev --workspace=@darshjdb/admin

# Type check
npm run typecheck --workspace=@darshjdb/admin

# Production build
npm run build --workspace=@darshjdb/admin
```

The dev server proxies `/api` requests to `http://localhost:7700` (the DarshJDB server). Make sure `ddb dev` is running.

## Project Structure

```
src/
  App.tsx              # Root layout with React Router
  main.tsx             # Entry point
  index.css            # Tailwind base + custom component classes
  types.ts             # Shared TypeScript interfaces
  components/
    Badge.tsx           # Status badge (7 color variants)
    CommandPalette.tsx  # Cmd+K command launcher with fuzzy search
    DataTable.tsx       # Generic sortable/paginated table
    Modal.tsx           # Accessible dialog component
    Sidebar.tsx         # Collapsible nav sidebar
    TopBar.tsx          # Header bar with status + actions
  lib/
    mock-data.ts        # Development mock data
    utils.ts            # Formatters (bytes, numbers, time, MIME)
  pages/
    AuthUsers.tsx       # User management page
    DataExplorer.tsx    # Data browser + DarshanQL query editor
    Functions.tsx       # Function registry + execution history
    Logs.tsx            # Log viewer with real-time streaming
    Schema.tsx          # Entity schema visualization
    Settings.tsx        # Configuration management
    Storage.tsx         # File storage browser
```

## Build Output

Production build goes to `dist/` with source maps enabled. The output is a static SPA that gets bundled into the DarshJDB server binary and served at `/admin`.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+K` | Open command palette |
| `Cmd+Enter` | Execute query (Data Explorer) |
| `Escape` | Close modal / palette |

## Documentation

- [Admin Dashboard Guide](../../docs/admin-dashboard.md)
- [Getting Started](../../docs/getting-started.md)
- [API Reference](../../docs/api-reference.md)
