# DarshJDB Developer Experience (DX) Strategy

**Author:** DX Research Audit  
**Date:** 2026-04-05  
**Scope:** End-to-end developer experience from discovery to production  
**Methodology:** Heuristic evaluation, cognitive walkthrough, API surface analysis

---

## Executive Summary

DarshJDB has strong architectural foundations and an ambitious SDK ecosystem. The README is compelling and the feature matrix positions the product well against Firebase, Supabase, InstantDB, and Convex. However, the developer experience has friction points that will determine whether the "three commands to real-time backend" promise converts into sustained adoption. This audit identifies 47 specific improvements across seven categories, prioritized by developer impact.

---

## 1. First 5 Minutes Experience

### 1.1 Current State Assessment

The onboarding flow today:

```
curl install script --> ddb dev --> copy React snippet --> hope it works
```

**Friction points identified:**

| # | Friction | Severity | Where |
|---|----------|----------|-------|
| F1 | Install script requires `curl | sh` (trust barrier) | High | README, getting-started.md |
| F2 | No `npx` / `brew` / `cargo install` alternatives listed | Medium | README |
| F3 | Docker is a hidden prerequisite for Postgres, not explicit until `ddb dev` runs | High | getting-started.md line 7 |
| F4 | `ddb dev` defaults to port 4820 (CLI) but README says 7700 --- contradictory | Critical | main.rs:41 vs README:117 |
| F5 | No guided interactive setup --- `ddb init` creates files silently with no prompts | Medium | main.rs:967-1049 |
| F6 | The React example in README uses a declarative query syntax (`db.useQuery({ todos: {...} })`) but the actual `useQuery` hook in code expects a `Query<T>` object with `collection`, `where` array, `orderBy` --- different API | Critical | README:129 vs use-query.ts:10-27 |
| F7 | No "hello world" verification --- developer has no way to confirm it worked | High | getting-started.md |
| F8 | `appId: 'my-app'` appears with no explanation of where this value comes from | Medium | getting-started.md:52 |

**F4 and F6 are showstoppers.** A developer following the README will hit errors within 60 seconds. The documented API and the actual API must be identical.

### 1.2 Ideal Onboarding Flow

Target: working real-time app in under 3 minutes, zero confusion.

```
                         DISCOVERY (30s)
                              |
                    npm create darshjdb@latest
                              |
                   +----------+----------+
                   |                     |
             Interactive            Quick start
             (prompts for           (ddb init
              framework,             --template react
              auth, features)        --no-prompt)
                   |                     |
                   +----------+----------+
                              |
                       ddb dev (auto)
                              |
                    +------ VERIFY ------+
                    |                    |
              Dashboard opens      Terminal shows:
              with sample data     "Connected! Try:
                                    curl localhost:4820/api/health"
                              |
                    SDK code already in project,
                    hot-reload running
                              |
                       BUILD FIRST FEATURE
                       (guided by dashboard
                        "Getting Started" tab)
```

### 1.3 Recommended: `npm create darshjdb@latest`

The single highest-impact DX improvement. A scaffolding tool that:

```bash
$ npm create darshjdb@latest

  DarshJDB  v0.1.0

  Where should we create your project?
  > ./my-app

  Which framework?
  > React  /  Next.js  /  Angular  /  Vue  /  Svelte  /  Vanilla  /  Server only

  Include authentication?
  > Yes (email + Google OAuth)  /  Yes (email only)  /  No

  Include example data (todos)?
  > Yes

  Installing dependencies...
  Creating darshan/ directory...
  Generating TypeScript types...
  Starting dev server...

  Done! Your DarshJDB project is ready.

    cd my-app
    ddb dev       (already running)
    
    Dashboard:  http://localhost:4820/admin
    API:        http://localhost:4820
    WebSocket:  ws://localhost:4820

  Next steps:
    1. Open the dashboard to see your data
    2. Edit src/App.tsx to see live reload
    3. Read the docs: https://db.darshj.me/docs
```

**Implementation:** Create `packages/create-darshjdb/` as a Node.js package using `@clack/prompts` (the library behind `create-svelte`, `create-astro`). Each template is a directory under `templates/{framework}/`. The scaffolder copies the template, runs string replacements for project name and config, installs deps, and optionally starts `ddb dev`.

### 1.4 Interactive Tutorial / Playground

**Browser playground at `db.darshj.me/playground`:**

```
+---------------------------------------------------------------+
|  DarshJDB Playground                          [Share] [Reset] |
+---------------------------------------------------------------+
|                        |                                       |
|   QUERY EDITOR         |   LIVE RESULTS                       |
|                        |                                       |
|   {                    |   [                                   |
|     todos: {           |     { id: "abc", title: "Learn...",   |
|       $where: {        |       done: false, createdAt: ... },  |
|         done: false    |     { id: "def", title: "Build...",   |
|       },               |       done: false, createdAt: ... }   |
|       $order: {        |   ]                                   |
|         createdAt:     |                                       |
|           "desc"       |   2 results | 1.2ms | tx: 42          |
|       }                |                                       |
|     }                  |                                       |
|   }                    |   --- MUTATION PANEL ---               |
|                        |                                       |
|   [Run Query]          |   db.transact(                        |
|   [Subscribe Live]     |     db.tx.todos[db.id()].set({        |
|                        |       title: "New todo",              |
|                        |       done: false                     |
|                        |     })                                |
|                        |   )                                   |
|                        |                                       |
|                        |   [Run Mutation]                      |
+---------------------------------------------------------------+
|  TIMELINE: shows real-time sync events as they happen          |
|  [ws] q-init  | [ws] q-diff +1  | [ws] q-diff ~1             |
+---------------------------------------------------------------+
```

**Implementation:** Use Sandpack (CodeSandbox's embeddable editor) with a hosted DarshJDB instance that resets every 30 minutes. The playground backend is a shared read-write DarshJDB server with ephemeral data, showing real-time sync across browser tabs.

### 1.5 `ddb init` Improvements

Current `ddb init` creates files but doesn't guide. Proposed enhancements:

```rust
// Add to Commands enum:
Init {
    /// Project name
    #[arg(default_value = ".")]
    name: String,
    
    /// Skip interactive prompts
    #[arg(long)]
    no_prompt: bool,
    
    /// Framework template
    #[arg(long, value_parser = ["react", "nextjs", "angular", "vue", "svelte", "vanilla", "server"])]
    template: Option<String>,
    
    /// Include auth scaffolding
    #[arg(long)]
    with_auth: bool,
    
    /// Include example data and seed file
    #[arg(long)]
    with_examples: bool,
}
```

After scaffolding, print a verification command:

```
  Verify your setup:
    curl http://localhost:4820/api/health | jq .
    
  Expected output:
    { "status": "healthy", "version": "0.1.0", ... }
```

---

## 2. SDK Ergonomics

### 2.1 API Surface Consistency Audit

The core promise of DarshJDB is "one API across every framework." The current reality:

| Concept | React | Angular | Vue | PHP | Python | Consistent? |
|---------|-------|---------|-----|-----|--------|-------------|
| Init | `DarshJDB.init({})` | `provideDarshan({})` | Not shown | `new Client([])` | `DarshJDB(url, key)` | NO |
| Query | `db.useQuery({...})` | `injectQuery({...})` | Not shown | `$db->query([...])` | `db.query({...})` | Partial |
| Mutation | `db.transact(db.tx...)` | Not shown | Not shown | `$db->transact([...])` | `db.transact([...])` | Partial |
| Auth | Not shown | Not shown | Not shown | Not shown | Not shown | N/A |

**Critical inconsistencies:**

1. **Init signature varies wildly.** PHP takes `serverUrl` + `apiKey` in a flat array. Python takes positional args. React/Angular take a config object with `appId`. Server SDKs need explicit URL; client SDKs may not. This is confusing but partially justified by platform conventions.

2. **Query syntax has two APIs.** The README and getting-started.md show a declarative object syntax (`{ todos: { $where: {...} } }`) that looks like InstantDB. But the actual `QueryBuilder` in `query.ts` uses a fluent builder (`db.query('users').where('age', '>=', 18)`), and `useQuery` in the React SDK expects a `Query<T>` type that doesn't match either documented syntax. **This is the single biggest DX problem in the codebase.**

3. **Transaction syntax differs.** React uses `db.transact(db.tx.todos[db.id()].set({...}))` (a Proxy-based builder like InstantDB). PHP and Python use raw arrays/dicts. These should feel like the same operation in different languages.

**Recommended fix:** The declarative query syntax shown in the README is the better developer experience. Implement it as the primary API and keep the fluent builder as an advanced escape hatch.

```typescript
// PRIMARY API (declarative, matches README)
const { data } = db.useQuery({
  todos: {
    $where: { done: false },
    $order: { createdAt: 'desc' },
    $limit: 20,
    owner: {}  // graph traversal
  }
});

// ADVANCED API (fluent builder, for programmatic queries)
const { data } = db.useQuery(
  db.query<Todo>('todos')
    .where('done', '=', false)
    .orderBy('createdAt', 'desc')
    .limit(20)
);
```

### 2.2 Error Messages Quality Audit

Current error handling in `client.ts`:

```typescript
// Line 295-298: Authentication failure
throw new Error(
  `Authentication failed: ${JSON.stringify(resp.payload)}`
);

// Line 315: Send failure
throw new Error('Cannot send: WebSocket is not open');

// Line 196: Timeout
reject(new Error(`Request ${id} timed out after ${timeoutMs}ms`));
```

**Problems:**

- `JSON.stringify(resp.payload)` dumps raw server response --- not actionable
- No error codes, just string messages
- No suggestions for resolution
- No links to documentation
- `console.warn('[DarshJDB] Server error:', msg.payload)` on line 347 silently drops errors

**Recommended: DarshanError class**

```typescript
export class DarshanError extends Error {
  constructor(
    public readonly code: string,       // "AUTH_FAILED", "CONN_TIMEOUT", etc.
    message: string,
    public readonly hint?: string,      // "Did you forget to call db.auth.signIn()?"
    public readonly docsUrl?: string,   // "https://db.darshj.me/docs/auth#troubleshooting"
    public readonly context?: Record<string, unknown>,
  ) {
    super(`[${code}] ${message}${hint ? `\n\n  Hint: ${hint}` : ''}`);
    this.name = 'DarshanError';
  }
}

// Usage:
throw new DarshanError(
  'AUTH_FAILED',
  'WebSocket authentication was rejected by the server.',
  'Check that your appId matches the one in your dashboard, and that your auth token is valid.',
  'https://db.darshj.me/docs/auth#troubleshooting',
  { appId: this.appId, serverUrl: this.serverUrl }
);
```

### 2.3 TypeScript Inference Quality

Current state: the generic `T = Record<string, unknown>` default means developers get no autocomplete unless they manually annotate types. With the declarative query syntax, TypeScript can infer the return shape from the query structure.

**Current (no inference):**
```typescript
const { data } = db.useQuery({ todos: { $where: { done: false } } });
// data is ReadonlyArray<Record<string, unknown>>
// data[0].title is `unknown` --- requires cast
```

**Target (full inference via `ddb pull` codegen):**
```typescript
// darshan/generated/schema.ts (auto-generated)
import type { DB } from '@darshjdb/react';

export interface Schema {
  todos: { id: string; title: string; done: boolean; createdAt: number; ownerId: string };
  users: { id: string; email: string; displayName: string };
}

// App code:
const db = DarshJDB.init<Schema>({ appId: 'my-app' });

const { data } = db.useQuery({ todos: { $where: { done: false } } });
// data.todos[0].title is `string` --- full autocomplete
// data.todos[0].nonexistent --- TypeScript error
```

**Implementation path:**
1. `ddb pull` already generates types (main.rs line 453-518). Extend it to generate a `Schema` interface.
2. Make `DarshJDB.init<S>()` accept a schema generic that flows through `useQuery`, `transact`, etc.
3. The `tx` proxy builder should be schema-aware: `db.tx.todos` only autocompletes valid entity names.

### 2.4 IntelliSense / Autocomplete Experience

The current `$where` syntax in the README uses plain objects, which TypeScript can type-check if we define mapped types:

```typescript
type WhereFilter<T> = {
  [K in keyof T]?: T[K] | { $gt?: T[K]; $gte?: T[K]; $lt?: T[K]; $lte?: T[K]; $in?: T[K][]; $contains?: string };
};

type QueryDef<S extends Record<string, unknown>> = {
  [Entity in keyof S]?: {
    $where?: WhereFilter<S[Entity]>;
    $order?: { [K in keyof S[Entity]]?: 'asc' | 'desc' };
    $limit?: number;
    $offset?: number;
    $select?: (keyof S[Entity])[];
  } & {
    // Graph traversal: reference fields expand to related entities
    [K in keyof S[Entity] as S[Entity][K] extends string ? K : never]?: QueryDef<S>;
  };
};
```

This gives developers autocomplete on entity names, field names in `$where`, valid operators per field type, and valid sort fields. It is the single most impactful TypeScript DX improvement.

---

## 3. CLI Experience

### 3.1 Command Discoverability

Current command list (from `clap` `Subcommand` enum):

```
dev, deploy, push, pull, seed, migrate, logs, auth, backup, restore, status, init
```

**Missing commands that developers expect:**

| Command | Purpose | Priority |
|---------|---------|----------|
| `ddb doctor` | Diagnose environment issues | P0 |
| `ddb console` | Interactive REPL for queries | P1 |
| `ddb open` | Open dashboard in browser | P1 |
| `ddb env` | Show/set environment variables | P2 |
| `ddb upgrade` | Self-update the CLI binary | P2 |
| `ddb reset` | Drop and recreate dev database | P2 |
| `ddb completion` | Generate shell completions (bash/zsh/fish) | P2 |

### 3.2 `ddb doctor` --- Diagnose Common Issues

This is the highest-value missing command. When things don't work, developers need a single command that tells them what's wrong.

```
$ ddb doctor

  DarshJDB Doctor  v0.1.0

  Checks:
    [PASS]  Rust toolchain installed (rustc 1.82.0)
    [PASS]  Node.js 18+ (v22.4.0)
    [PASS]  Docker running (Docker 27.0.3)
    [FAIL]  PostgreSQL connection
              Cannot connect to postgres://localhost:5432
              Hint: Run `docker compose up -d` or `ddb dev` to start Postgres
    [PASS]  Port 4820 available
    [PASS]  ddb.toml found
    [WARN]  No .env file found
              Hint: Copy .env.example to .env for local config
    [PASS]  darshan/functions/ directory exists (3 functions)
    [PASS]  SDK versions compatible
              @darshjdb/react 0.1.0 <-> server 0.1.0

  Result: 1 failure, 1 warning

  Fix the failure above, then run `ddb doctor` again.
```

**Implementation sketch (Rust):**

```rust
Commands::Doctor => {
    println!("\n  {} DarshJDB Doctor\n", ">>>".bright_cyan().bold());
    
    let checks = vec![
        check_rust_toolchain().await,
        check_node_version().await,
        check_docker().await,
        check_postgres_connection(&cfg).await,
        check_port_available(cfg.port).await,
        check_config_file().await,
        check_env_file().await,
        check_functions_dir().await,
        check_sdk_compatibility(&cfg).await,
    ];
    
    for check in &checks {
        match check.status {
            CheckStatus::Pass => println!("    {} {}", "[PASS]".bright_green(), check.name),
            CheckStatus::Warn => {
                println!("    {} {}", "[WARN]".bright_yellow(), check.name);
                if let Some(hint) = &check.hint {
                    println!("              {}", hint.dimmed());
                }
            }
            CheckStatus::Fail => {
                println!("    {} {}", "[FAIL]".bright_red(), check.name);
                println!("              {}", check.detail.bright_red());
                if let Some(hint) = &check.hint {
                    println!("              Hint: {}", hint.bright_yellow());
                }
            }
        }
    }
}
```

### 3.3 Output Formatting Assessment

The current CLI output uses `colored` and `indicatif` well. The `>>>` prefix and `-->` result indicators create a consistent visual language. Improvements:

1. **Add `--json` flag to all commands** for CI/scripting:
   ```bash
   ddb status --json | jq .version
   ```

2. **Add `--quiet` flag** that suppresses decorative output:
   ```bash
   ddb push --quiet  # only errors go to stderr
   ```

3. **Table formatting for `auth list-users`** --- currently dumps raw JSON. Use `comfy-table` or `tabled` crate:
   ```
     ID                  Email              Roles      Last Login
     usr_abc123          admin@co.com       admin      2h ago
     usr_def456          dev@co.com         editor     3d ago
   ```

4. **Migration status table** --- currently dumps raw JSON (main.rs line 597). Format as a proper table:
   ```
     Migration                    Status      Applied At
     001_initial_schema.sql       Applied     2026-04-01 10:30
     002_add_indexes.sql          Applied     2026-04-02 14:15
     003_user_roles.sql           Pending     -
   ```

### 3.4 Progress Indicators

Current implementation is solid --- `indicatif` spinners for single operations, progress bars for batch operations. One gap: **`ddb dev` has no startup progress.** The developer sees "Starting server..." and then waits with no feedback on what's happening (Postgres startup, schema creation, function compilation, etc.).

Proposed startup sequence:

```
  >>> DarshJDB dev server

    [1/5] Checking Postgres...          done (Docker, pg16)
    [2/5] Applying schema...            done (4 entity types)
    [3/5] Loading functions...           done (3 functions)
    [4/5] Starting WebSocket server...   done
    [5/5] Opening dashboard...           done

  --> Listening on http://localhost:4820
  --> Dashboard at http://localhost:4820/admin
  --> Watching for changes (hot-reload enabled)

  Ready in 2.4s
```

---

## 4. Admin Dashboard UX

### 4.1 Current Strengths

- Dark-first design with amber accent --- visually cohesive
- Component library is clean (Badge, DataTable, Modal, CommandPalette, Sidebar, TopBar)
- Seven page sections cover the essential admin tasks
- Live mode toggle in DataExplorer and Logs
- Lucide icons used consistently
- Filter/search in entity list

### 4.2 Current Gaps

| Gap | Severity | Page |
|-----|----------|------|
| All data is mock (`mockEntityTypes`, `mockRecords`, `mockLogs`) --- no real API integration | Critical | All pages |
| DataExplorer has no inline editing | High | DataExplorer |
| No bulk operations (delete selected, update field across rows) | High | DataExplorer |
| Schema page uses static `relationships` array, not derived from data | High | Schema |
| No query builder UI --- only raw SQL input | Medium | DataExplorer |
| No real-time event stream visualization | Medium | Logs |
| No keyboard shortcuts beyond CommandPalette | Low | All |

### 4.3 Data Explorer Improvements

**Inline Editing:**

```
+------------------------------------------------------------------+
| _id          | title             | done    | createdAt            |
|--------------|-------------------|---------|----------------------|
| abc123       | Learn DarshJDB   | [x]     | 2026-04-01 10:30     |
| def456       | Build app         | [ ]     | 2026-04-02 14:15     |
|              |                   |         |                      |
| ghi789       | Write tests       | [ ]     | 2026-04-03 09:00     |
|              |  ^                |         |                      |
|              |  Click to edit    |         |                      |
|              |  inline. Esc to   |         |                      |
|              |  cancel. Enter    |         |                      |
|              |  to save.         |         |                      |
+------------------------------------------------------------------+
| [+Add Row]  | Selected: 2  [Delete]  [Bulk Edit]  [Export CSV]   |
+------------------------------------------------------------------+
```

Implementation: Each cell becomes an editable input on click. Changes are batched and committed on blur/Enter. Checkbox fields toggle immediately. A yellow left-border indicates unsaved changes. Bulk operations appear in a bottom toolbar when rows are selected via checkboxes.

**Bulk Operations Panel:**

```
+------------------------------------------------------------------+
|  2 rows selected                                                  |
|                                                                   |
|  [Set Field]  field: [done  v]  value: [true  v]   [Apply]       |
|  [Delete Selected]                                                |
|  [Export Selected as JSON]                                        |
+------------------------------------------------------------------+
```

### 4.4 Visual Query Builder

Replace the raw SQL textarea with a structured query builder for non-SQL users:

```
+------------------------------------------------------------------+
|  QUERY BUILDER                                          [<> SQL]  |
+------------------------------------------------------------------+
|                                                                   |
|  FROM  [ todos          v ]                                       |
|                                                                   |
|  WHERE                                                            |
|    [ done       v ]  [ equals    v ]  [ false  v ]   [x] [+]     |
|    [ createdAt  v ]  [ after     v ]  [ 2026-01-01 ] [x] [+]     |
|                                                                   |
|  ORDER BY                                                         |
|    [ createdAt  v ]  [ desc  v ]                     [x] [+]     |
|                                                                   |
|  LIMIT  [ 50 ]   OFFSET  [ 0 ]                                   |
|                                                                   |
|  INCLUDE RELATIONS                                                |
|    [x] owner (users)                                              |
|    [ ] tags (tags)                                                |
|                                                                   |
|  [Run Query]  [Save as View]  [Copy as SDK Code]                 |
+------------------------------------------------------------------+
```

The `[<> SQL]` toggle shows the equivalent DarshanQL. `[Copy as SDK Code]` generates the exact React/Angular/Vue code to execute this query --- a powerful learning tool.

### 4.5 Schema Visualization Improvements

The current Schema page uses a static grid of cards. Replace with an interactive ERD:

```
+------------------------------------------------------------------+
|  SCHEMA DIAGRAM                    [Zoom +] [Zoom -] [Fit] [PNG] |
+------------------------------------------------------------------+
|                                                                   |
|  +----------+         +----------+         +-----------+          |
|  | users    |         | todos    |         | files     |          |
|  |----------|    1:N  |----------|         |-----------|          |
|  | id    PK |<--------| ownerId  |         | id     PK |          |
|  | email    |         | title    |         | path      |          |
|  | name     |         | done     |         | size      |          |
|  | avatar   |         | created  |    1:N  | uploadedBy|------+   |
|  +----------+         +----------+         +-----------+      |   |
|       |                                                       |   |
|       +-------------------------------------------------------+   |
|                                                                   |
|  Drag entities to rearrange. Click a field for details.           |
|  Click a relationship line to see the FK definition.              |
+------------------------------------------------------------------+
```

Use `reactflow` or `elkjs` for the layout engine. Entities are draggable nodes. Relationship lines are labeled with cardinality. Clicking a field opens a detail panel showing type, constraints, indexes, and sample values.

### 4.6 Real-Time Log Viewer Enhancements

Current Logs page has search, level filter, and live mode. Add:

1. **Structured JSON expansion** --- click a log entry to see the full structured payload with syntax highlighting
2. **Trace correlation** --- group logs by request ID so you can see the full lifecycle of a mutation (receive -> validate -> authorize -> write -> broadcast)
3. **Sparkline timeline** --- a mini chart at the top showing log volume per minute, colored by level
4. **Filter by entity type** --- "Show me all mutations to the `todos` table in the last hour"
5. **Tail with grep** --- real-time log stream filtered by regex pattern

```
+------------------------------------------------------------------+
|  LOGS                                  [Live: ON]  [Download]     |
+------------------------------------------------------------------+
|  [____Filter regex____]  Level: [All v]  Entity: [All v]          |
|                                                                   |
|  --- Volume (last 30m) ---                                        |
|  ||||| ||  |||||| ||||||||||||| ||| |||| |||||||| ||||||||||||     |
|  ^^^^^ = error spike at 14:23                                     |
+------------------------------------------------------------------+
|  14:25:03  INFO   query    todos      1.2ms   usr_abc  req_x1    |
|  14:25:04  INFO   mutate   todos      3.4ms   usr_abc  req_x2    |
|  > 14:25:04  DEBUG  validate   passed (RLS: owner = usr_abc)      |
|  > 14:25:04  DEBUG  write      1 triple inserted                  |
|  > 14:25:04  DEBUG  broadcast  2 subscribers notified (0.3ms)     |
|  14:25:05  WARN   auth     -          -       anon     req_x3    |
|    "Anonymous connection rejected (appId: unknown-app)"           |
|    Hint: Check your DarshJDB.init({ appId }) matches dashboard   |
+------------------------------------------------------------------+
```

---

## 5. Documentation UX

### 5.1 Information Architecture Audit

Current doc structure (from `docs/` listing):

```
docs/
  README.md              (index)
  getting-started.md     (onboarding)
  architecture.md
  authentication.md
  permissions.md
  query-language.md
  server-functions.md
  presence.md
  storage.md
  self-hosting.md
  performance.md
  security.md
  migration.md
  api-reference.md
  SECURITY_AUDIT.md
  guide/                 (subdirectory)
  strategy/              (this document)
```

**Gaps in information architecture:**

| Missing Doc | Priority | Notes |
|-------------|----------|-------|
| Tutorials (step-by-step builds) | P0 | "Build a chat app in 15 minutes" |
| Recipes / Cookbook | P1 | Common patterns: pagination, auth guards, file upload |
| Error reference | P1 | Searchable error code registry |
| CLI reference | P1 | Auto-generated from clap `--help` |
| SDK API reference | P0 | Auto-generated from TSDoc/PHPDoc/pydoc |
| Changelog (linked) | P2 | CHANGELOG.md exists but not linked from docs |
| Troubleshooting (dedicated) | P1 | Referenced in getting-started.md but doesn't exist |
| Deployment guides (per platform) | P2 | Railway, Fly.io, Hetzner, DigitalOcean, AWS |

**Recommended structure:**

```
docs/
  index.md                    "What is DarshJDB?"
  getting-started/
    installation.md
    quickstart-react.md
    quickstart-nextjs.md
    quickstart-angular.md
    quickstart-vue.md
    quickstart-php.md
    quickstart-python.md
  concepts/
    architecture.md
    data-model.md             (triple store, EAV, schema-on-read)
    query-language.md
    real-time-sync.md
    offline-first.md
  guides/
    authentication.md
    permissions.md
    server-functions.md
    storage.md
    presence.md
    migrations.md
    deployment/
      docker.md
      kubernetes.md
      railway.md
      fly-io.md
  tutorials/
    build-todo-app.md
    build-chat-app.md
    build-kanban-board.md
    multi-tenant-saas.md
  reference/
    cli.md                    (auto-generated from clap)
    rest-api.md               (auto-generated from OpenAPI)
    sdk/
      react.md                (auto-generated from TSDoc)
      nextjs.md
      angular.md
      vue.md
      php.md
      python.md
    error-codes.md
    config.md                 (ddb.toml reference)
    wire-protocol.md
  troubleshooting.md
  performance.md
  security.md
  changelog.md
```

### 5.2 Search Functionality

**Recommended: Algolia DocSearch** (free for open source projects)

Apply at https://docsearch.algolia.com/apply/. Provides:
- Instant search-as-you-type across all docs
- Keyboard shortcut (Cmd+K) integration
- Hierarchical results (concept > guide > API reference)
- Analytics on what developers search for (invaluable for prioritizing doc improvements)

**Alternative for self-hosted:** Pagefind (static search, zero dependencies, runs at build time). Ideal if the docs are built with Astro/Starlight or VitePress.

### 5.3 Code Examples Quality

**Current strengths:**
- React and Next.js examples are good
- PHP and Python examples cover basic CRUD
- cURL examples are useful

**Gaps:**
- No examples show error handling
- No examples show TypeScript generics usage
- No examples show offline-first behavior
- No examples show permission rules in context
- No copy-to-clipboard buttons in docs
- No "Run in playground" buttons

**Every code example should follow this template:**

```
[ React | Next.js | Angular | Vue | PHP | Python | cURL ]   <-- framework tabs

```tsx
import { DarshJDB } from '@darshjdb/react';

const db = DarshJDB.init({ appId: 'my-app' });

function UserList() {
  const { data, isLoading, error } = db.useQuery({
    users: {
      $where: { active: true },
      $order: { createdAt: 'desc' },
      $limit: 20,
    },
  });

  // Always handle errors
  if (error) {
    console.error(error.code, error.hint);
    return <ErrorBoundary error={error} />;
  }

  if (isLoading) return <Skeleton count={5} />;

  return (
    <ul>
      {data.users.map(user => (
        <li key={user.id}>{user.displayName}</li>
      ))}
    </ul>
  );
}
```

[Copy] [Open in Playground]
```

### 5.4 Interactive Code Playground (Sandpack)

Embed Sandpack-powered code playgrounds directly in documentation pages. Developers edit code and see results live without leaving the docs.

```
+------------------------------------------------------------------+
|  Live Example: Real-Time Todo List                                |
+------------------------------------------------------------------+
|  CODE EDITOR                    |  PREVIEW                        |
|                                 |                                  |
|  function TodoApp() {           |  +----------------------------+  |
|    const { data } = db.useQ..  |  | [ ] Learn DarshJDB        |  |
|                                 |  | [ ] Build something great  |  |
|    return (                     |  | [Add Todo: ___________]    |  |
|      <ul>                       |  +----------------------------+  |
|        {data.todos.map(...      |                                  |
|  }                              |  Console:                        |
|                                 |  > Connected to DarshJDB       |
|  [Reset] [Fork]                 |  > Query subscribed (2 results) |
+------------------------------------------------------------------+
```

**Implementation:** Use `@codesandbox/sandpack-react` with a custom DarshJDB template. The template pre-installs `@darshjdb/react` and connects to a shared playground server. Each tutorial page gets an embedded playground with the relevant example pre-loaded.

### 5.5 API Reference Auto-Generation

| SDK | Source Format | Tool | Output |
|-----|--------------|------|--------|
| TypeScript (`@darshjdb/client`, `@darshjdb/react`, etc.) | TSDoc comments | `typedoc` | HTML/Markdown |
| PHP | PHPDoc | `phpDocumentor` | HTML/Markdown |
| Python | Google-style docstrings | `sphinx` + `autodoc` | HTML/Markdown |
| REST API | OpenAPI spec (server-generated) | `redocly` or `scalar` | Interactive HTML |
| CLI | `clap` `--help` output | Custom script or `clap-markdown` | Markdown |

The TypeScript SDKs already have good TSDoc comments (visible in `client.ts`, `query.ts`, `use-query.ts`). Running `typedoc` today would produce usable API reference with minimal effort.

---

## 6. Error Experience

### 6.1 Error Code Registry

Define a structured error code system with consistent naming:

```
DDB-{CATEGORY}-{NUMBER}

Categories:
  AUTH    Authentication and authorization
  CONN    Connection and transport
  QUERY   Query parsing and execution
  TX      Transaction and mutation
  PERM    Permission evaluation
  FUNC    Server function execution
  STORE   Storage operations
  SCHEMA  Schema validation
  CONFIG  Configuration
```

**Error code registry (initial set):**

| Code | Message | Hint | Docs Link |
|------|---------|------|-----------|
| DDB-AUTH-001 | Authentication failed | Check appId and auth token | /docs/auth#troubleshooting |
| DDB-AUTH-002 | Token expired | Call db.auth.refresh() or re-authenticate | /docs/auth#token-refresh |
| DDB-AUTH-003 | Invalid OAuth provider | Supported: google, github, apple, discord | /docs/auth#oauth |
| DDB-CONN-001 | WebSocket connection refused | Is the server running? Try `ddb status` | /docs/troubleshooting#connection |
| DDB-CONN-002 | Connection timeout | Server may be overloaded or unreachable | /docs/troubleshooting#timeout |
| DDB-CONN-003 | Protocol version mismatch | Update your SDK: npm update @darshjdb/react | /docs/troubleshooting#version |
| DDB-QUERY-001 | Unknown entity type | Entity "{name}" not found. Available: {list} | /docs/query-language#entities |
| DDB-QUERY-002 | Invalid where clause | Field "{field}" does not exist on "{entity}" | /docs/query-language#where |
| DDB-QUERY-003 | Query too complex | Reduce nesting depth or add $limit | /docs/performance#query-limits |
| DDB-TX-001 | Transaction conflict | Another transaction modified this entity | /docs/concepts/offline-first#conflicts |
| DDB-TX-002 | Validation failed | Field "{field}": {reason} | /docs/concepts/data-model#validation |
| DDB-PERM-001 | Permission denied (table) | Role "{role}" cannot access "{entity}" | /docs/permissions#table-rules |
| DDB-PERM-002 | Permission denied (row) | RLS policy filtered this entity | /docs/permissions#row-level |
| DDB-PERM-003 | Permission denied (field) | Field "{field}" is restricted for role "{role}" | /docs/permissions#field-level |
| DDB-FUNC-001 | Function not found | No function named "{name}". Run `ddb push` | /docs/server-functions#deploy |
| DDB-FUNC-002 | Function timeout | Exceeded {limit}ms. Optimize or increase timeout | /docs/server-functions#limits |
| DDB-FUNC-003 | Function runtime error | {stack trace} | /docs/server-functions#debugging |

### 6.2 "Did You Mean?" Suggestions

When a developer references an entity or field that doesn't exist, compute Levenshtein distance against known names:

```typescript
// Developer writes:
db.useQuery({ todo: { $where: { done: false } } });

// Error output:
// [DDB-QUERY-001] Unknown entity type "todo".
//
//   Did you mean "todos"?
//
//   Available entity types: todos, users, sessions, files
//   Docs: https://db.darshj.me/docs/query-language#entities
```

**Implementation:** Levenshtein distance with a threshold of 2 edits. If exactly one candidate is within threshold, show "Did you mean?". If multiple, show "Similar: ...". This applies to entity names, field names, function names, and CLI subcommands.

For the CLI, `clap` already provides "Did you mean?" for subcommands. Extend this to flag values:

```bash
$ ddb logs --level warning
error: invalid value "warning" for --level
  Did you mean "warn"?
  Valid levels: debug, info, warn, error
```

### 6.3 Documentation Links in Errors

Every error message should include a URL. In the terminal, make it clickable (most terminals support OSC 8 hyperlinks):

```rust
// In CLI error output:
eprintln!(
    "  {} {}\n  {} {}\n  {} {}",
    "Error:".bright_red().bold(),
    "PostgreSQL connection failed",
    "Hint:".bright_yellow(),
    "Run `docker compose up -d` or provide --database-url",
    "Docs:".dimmed(),
    "https://db.darshj.me/docs/troubleshooting#postgres".underline()
);
```

In the SDK, errors include `docsUrl` which can be logged or displayed:

```typescript
try {
  await db.connect();
} catch (err) {
  if (err instanceof DarshanError) {
    console.error(`${err.code}: ${err.message}`);
    console.error(`Fix: ${err.hint}`);
    console.error(`Docs: ${err.docsUrl}`);
  }
}
```

### 6.4 Debug Mode

Add a `debug` option to the client config that enables verbose logging:

```typescript
const db = DarshJDB.init({
  appId: 'my-app',
  debug: true, // or process.env.NODE_ENV === 'development'
});

// With debug: true, the SDK logs:
// [DarshJDB:conn] Connecting to ws://localhost:4820/v1/apps/my-app/ws
// [DarshJDB:conn] WebSocket opened, authenticating...
// [DarshJDB:auth] Sending anonymous auth for appId=my-app
// [DarshJDB:auth] Auth OK (user: null, anonymous: true)
// [DarshJDB:conn] Connected (latency: 3ms)
// [DarshJDB:query] Subscribe { todos: { $where: { done: false } } }  hash=a1b2c3
// [DarshJDB:query] q-init received (2 results, tx=42, 1.2ms)
// [DarshJDB:sync] Mutation: set todos/abc123 { done: true }
// [DarshJDB:sync] Optimistic update applied
// [DarshJDB:sync] Server confirmed tx=43 (3.4ms)
// [DarshJDB:sync] q-diff received: 1 updated, 0 added, 0 removed
```

This eliminates the "what is happening internally?" question that drives developers to give up. Debug mode should be the default in development and disabled in production.

---

## 7. Community Experience

### 7.1 Discord Server Structure

Recommended channel layout:

```
DARSHANDB DISCORD
|
+-- WELCOME
|   #welcome-rules          (read-only, rules + links)
|   #introductions           (new members introduce themselves)
|   #announcements           (releases, blog posts, events)
|
+-- HELP
|   #getting-started         (onboarding questions)
|   #react-sdk               (React-specific help)
|   #nextjs-sdk              (Next.js-specific help)
|   #angular-sdk
|   #server-sdks             (PHP, Python, Node admin)
|   #self-hosting            (Docker, K8s, VPS deployment)
|   #permissions             (RLS, ABAC, field-level)
|   #server-functions        (V8 runtime, cron, actions)
|
+-- DISCUSSION
|   #feature-requests        (ideas + voting via emoji)
|   #show-and-tell           (apps built with DarshJDB)
|   #architecture            (deep technical discussion)
|   #off-topic
|
+-- CONTRIBUTING
|   #development             (core dev discussion)
|   #pull-requests           (GitHub PR notifications via webhook)
|   #ci-status               (CI/CD notifications)
|
+-- VOICE
|   Office Hours             (weekly community call)
```

**Key automation:**
- GitHub webhook posts new issues and PRs to `#pull-requests`
- Bot auto-tags threads by SDK framework
- New members get a welcome DM with links to quickstart and FAQ
- Solved questions get archived weekly into a searchable FAQ

### 7.2 GitHub Discussions vs Issues

**Issues:** Bug reports and tracked feature requests only. Use issue templates:

```
.github/ISSUE_TEMPLATE/
  bug_report.yml           (structured: steps to reproduce, expected, actual, environment)
  feature_request.yml      (structured: problem, proposed solution, alternatives)
  sdk_bug.yml              (SDK-specific: package, version, framework, code snippet)
```

**Discussions:** Everything else:

```
Categories:
  Q&A                 (questions, "how do I...?")
  Ideas               (brainstorming, not yet ready for feature request)
  Show and Tell       (apps, tutorials, blog posts)
  Announcements       (releases, breaking changes)
```

**Triage automation:** Use GitHub Actions to auto-label issues by path mention (`sdk:react`, `cli`, `server`, `admin`, `docs`). Issues without reproduction steps get a bot comment asking for more details and are labeled `needs-info`.

### 7.3 Contribution Workflow for New Contributors

CONTRIBUTING.md should include a "Good First Issues" section. Label issues with `good-first-issue` and `help-wanted`. Create a contribution ladder:

```
CONTRIBUTION LADDER

Level 1: First Contribution
  - Fix a typo in docs
  - Add a code example
  - Improve an error message
  Labels: good-first-issue, docs, dx

Level 2: Feature Contribution
  - Add a CLI command
  - Write a tutorial
  - Add SDK tests
  Labels: help-wanted, enhancement

Level 3: Core Contribution
  - Server-side feature (Rust)
  - SDK architecture change
  - Performance optimization
  Labels: core, needs-design-review
```

**Dev setup should be one command:**

```bash
git clone https://github.com/darshjme/darshjdb.git
cd darshjdb
make dev-setup    # installs Rust, Node, starts Postgres, runs tests
```

Or, for the devcontainer crowd:

```json
// .devcontainer/devcontainer.json
{
  "name": "DarshJDB Dev",
  "image": "mcr.microsoft.com/devcontainers/rust:1",
  "features": {
    "ghcr.io/devcontainers/features/node:1": { "version": "22" },
    "ghcr.io/devcontainers/features/docker-in-docker:2": {}
  },
  "postCreateCommand": "make dev-setup",
  "forwardPorts": [4820]
}
```

### 7.4 Showcase / Gallery

Create a `db.darshj.me/showcase` page where developers submit their apps:

```
+------------------------------------------------------------------+
|  Built with DarshJDB                                             |
+------------------------------------------------------------------+
|                                                                   |
|  +-------------+  +-------------+  +-------------+               |
|  | [Screenshot]|  | [Screenshot]|  | [Screenshot]|               |
|  |             |  |             |  |             |               |
|  | TeamSync    |  | InvoiceBot  |  | QuizMaster  |               |
|  | Real-time   |  | SaaS with   |  | Live quiz   |               |
|  | team collab |  | multi-tenant|  | app for     |               |
|  |             |  | billing     |  | classrooms  |               |
|  | React       |  | Next.js     |  | Vue + PHP   |               |
|  | @johndoe    |  | @startup_co |  | @teacher_x  |               |
|  +-------------+  +-------------+  +-------------+               |
|                                                                   |
|  [Submit Your App]                                                |
+------------------------------------------------------------------+
```

Submissions via GitHub PR to a `showcase/` directory (JSON files with metadata + screenshot URL). A build step generates the gallery page. This provides social proof and gives developers recognition.

---

## Priority Roadmap

### Phase 1: Critical Fixes (Week 1-2)

| # | Item | Impact | Effort |
|---|------|--------|--------|
| 1 | Fix port inconsistency (README says 7700, CLI defaults 4820) | Critical | 1h |
| 2 | Reconcile query API (README declarative vs SDK builder) | Critical | 3d |
| 3 | Wire admin dashboard to real API (replace mock data) | Critical | 1w |
| 4 | Create `DarshanError` class with codes and hints | High | 2d |
| 5 | Fix `ddb init` to match the framework the dev is using | High | 2d |

### Phase 2: High-Impact DX (Week 3-6)

| # | Item | Impact | Effort |
|---|------|--------|--------|
| 6 | `npm create darshjdb@latest` scaffolder | High | 1w |
| 7 | `ddb doctor` command | High | 3d |
| 8 | Schema-aware TypeScript generics (`DarshJDB.init<Schema>`) | High | 1w |
| 9 | Error code registry (all 20+ codes) | High | 3d |
| 10 | Debug mode for SDK | Medium | 2d |
| 11 | `--json` and `--quiet` flags on all CLI commands | Medium | 2d |
| 12 | Auto-generated API reference (typedoc + sphinx + clap-markdown) | High | 3d |

### Phase 3: Polish and Community (Week 7-12)

| # | Item | Impact | Effort |
|---|------|--------|--------|
| 13 | Interactive playground at db.darshj.me/playground | High | 2w |
| 14 | Sandpack-embedded docs examples | Medium | 1w |
| 15 | Visual query builder in admin dashboard | Medium | 1w |
| 16 | Inline editing in Data Explorer | Medium | 1w |
| 17 | Interactive ERD in Schema page (reactflow) | Medium | 1w |
| 18 | Enhanced log viewer (trace correlation, sparklines) | Low | 1w |
| 19 | Discord server setup + automation | Medium | 2d |
| 20 | Showcase gallery | Low | 2d |
| 21 | Tutorial series (3 step-by-step guides) | High | 1w |
| 22 | Algolia DocSearch integration | Medium | 1d |
| 23 | Devcontainer + Makefile for contributors | Medium | 1d |
| 24 | Framework-specific quickstart docs (6 pages) | High | 3d |

---

## Metrics to Track

| Metric | Target | How to Measure |
|--------|--------|----------------|
| Time to first query | < 3 minutes | User testing, onboarding analytics |
| `ddb doctor` pass rate | > 90% on first try | CLI telemetry (opt-in) |
| Docs search success rate | > 80% | Algolia analytics |
| Error resolution rate | > 70% self-serve | Error code page views vs support tickets |
| NPM install to working app | < 5 minutes | User testing |
| GitHub issue response time | < 24 hours | GitHub metrics |
| Discord question answer time | < 4 hours | Bot tracking |
| Showcase submissions | > 10 in first 3 months | Gallery entries |

---

## Appendix A: Competitive DX Benchmarks

| Aspect | Firebase | Supabase | Convex | DarshJDB (target) |
|--------|----------|----------|--------|---------------------|
| Time to first query | ~2 min | ~5 min | ~3 min | < 3 min |
| CLI doctor command | No | No | No | Yes |
| Interactive playground | Yes (console) | Yes (SQL editor) | Yes (dashboard) | Yes (playground + dashboard) |
| Error codes | Yes | Partial | Yes | Yes |
| TypeScript inference | Moderate | Good (via codegen) | Excellent | Excellent (via codegen) |
| Offline-first docs | Poor | None | None | Comprehensive |
| Self-host docs | N/A | Good | N/A | Excellent |

## Appendix B: User Personas for DX Prioritization

**Persona 1: "Quick Start Quinn"** --- Junior dev, follows README literally, expects things to work on first try. Blocked by F4 (port mismatch) and F6 (API mismatch). Needs `npm create darshjdb@latest`.

**Persona 2: "Enterprise Eva"** --- Senior dev evaluating DarshJDB for team adoption. Needs TypeScript inference, error codes, self-hosting docs, security audit docs. Will judge DX by the quality of error messages and API reference.

**Persona 3: "Indie Dev Idris"** --- Solo developer in Lagos shipping a SaaS on a $5 VPS. Needs `ddb doctor`, clear self-hosting guides, offline-first documentation, and the showcase gallery for inspiration.

**Persona 4: "Contributor Carlos"** --- Open source enthusiast in Sao Paulo. Needs devcontainer, good-first-issue labels, contribution ladder, and a welcoming Discord.

---

*This document should be revisited quarterly as the product matures. DX is not a feature --- it is the product.*
