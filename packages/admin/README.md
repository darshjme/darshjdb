# @darshan/admin

Admin dashboard for DarshanDB -- data explorer, schema viewer, and management console.

## Stack

- **React 18** with TypeScript
- **Vite 6** for dev/build
- **Tailwind CSS 3** for styling
- **Radix UI** for accessible primitives (dialog, dropdown, select, switch, tabs, tooltip, popover)
- **Recharts** for data visualizations
- **Lucide React** for icons
- **React Router 6** for client-side routing

## Pages

| Route | Component | Description |
|-------------|-----------------|----------------------------------------------|
| `/` | DataExplorer | Browse entities, run DarshanQL queries |
| `/schema` | Schema | View entity types, fields, and relationships |
| `/functions` | Functions | Function registry, execution chart, history |
| `/auth` | AuthUsers | User management, sessions, permissions |
| `/storage` | Storage | File browser with grid/list view, drag-drop |
| `/logs` | Logs | Real-time log viewer with level filtering |
| `/settings` | Settings | Env vars, backups, rate limits, webhooks |

## Components

- **Sidebar** -- Collapsible navigation with active route highlighting
- **TopBar** -- Title, connection status indicator, command palette trigger, notifications
- **CommandPalette** -- `Cmd+K` fuzzy search across all pages (keyboard navigable)
- **DataTable** -- Generic sortable, paginated table with type-aware cell rendering
- **Modal** -- Accessible dialog with Escape-to-close and backdrop click
- **Badge** -- Color-coded status/label indicator (7 variants)

## Development

```bash
# From monorepo root
pnpm install

# Start dev server (port 3100)
pnpm --filter @darshan/admin dev

# Type check
pnpm --filter @darshan/admin typecheck

# Production build
pnpm --filter @darshan/admin build
```

The dev server proxies `/api` requests to `http://localhost:4000` (the DarshanDB server).

## Project Structure

```
src/
  App.tsx              # Root layout with routing
  main.tsx             # Entry point
  index.css            # Tailwind base + custom component classes
  types.ts             # Shared TypeScript interfaces
  components/
    Badge.tsx           # Status badge (7 color variants)
    CommandPalette.tsx  # Cmd+K command launcher
    DataTable.tsx       # Generic sortable/paginated table
    Modal.tsx           # Accessible dialog component
    Sidebar.tsx         # Collapsible nav sidebar
    TopBar.tsx          # Header bar with status + actions
  lib/
    mock-data.ts        # Development mock data
    utils.ts            # Formatters (bytes, numbers, time, MIME)
  pages/
    AuthUsers.tsx       # User management page
    DataExplorer.tsx    # Data browser + query editor
    Functions.tsx       # Function registry + execution history
    Logs.tsx            # Log viewer with real-time mode
    Schema.tsx          # Entity schema visualization
    Settings.tsx        # Configuration management
    Storage.tsx         # File storage browser
```

## Build Output

Production build goes to `dist/` with source maps enabled. The output is a static SPA suitable for deployment behind any HTTP server.
