# Angular SDK

The `@darshjdb/angular` package provides two reactive query APIs -- Angular Signals (17+) and RxJS Observables -- along with route guards, an HTTP interceptor, SSR support, and a standalone provider function. No NgModules required.

## Installation

```bash
npm install @darshjdb/angular @darshjdb/client
```

## Provider Setup

Use `provideDarshan()` in your application bootstrap. It registers the client, opens the connection during `APP_INITIALIZER`, and disconnects on window unload.

```typescript
// main.ts
import { bootstrapApplication } from '@angular/platform-browser';
import { provideDarshan } from '@darshjdb/angular';
import { AppComponent } from './app.component';

bootstrapApplication(AppComponent, {
  providers: [
    provideDarshan({
      serverUrl: 'https://db.example.com',
      appId: 'my-app',
    }),
  ],
});
```

### Configuration Options

```typescript
provideDarshan({
  serverUrl: 'https://db.example.com',
  appId: 'my-app',
  debug: true,
  reconnectInterval: 5_000,
  maxReconnectAttempts: 10,
});
```

`provideDarshan()` returns `EnvironmentProviders`, which can also be used in lazy-loaded route `providers` arrays for feature-scoped clients.

### Injection Tokens

The SDK exposes two tokens for direct injection:

- `DDB_CLIENT` -- the DarshJDB client instance
- `DDB_CONFIG` -- the configuration object passed to `provideDarshan()`

```typescript
import { inject } from '@angular/core';
import { DDB_CLIENT } from '@darshjdb/angular';

const client = inject(DDB_CLIENT);
```

## Signal-Based Queries (Angular 17+)

`darshanQuery()` subscribes to a live DarshJDB query and exposes the result as independent Angular Signals. Designed for zoneless / `OnPush` components.

```typescript
import { Component } from '@angular/core';
import { darshanQuery } from '@darshjdb/angular';

interface Todo {
  id: string;
  title: string;
  done: boolean;
}

@Component({
  template: `
    @if (todos.isLoading()) {
      <spinner />
    } @else if (todos.error()) {
      <error [message]="todos.error()!.message" />
    } @else {
      @for (todo of todos.data(); track todo.id) {
        <todo-item [todo]="todo" />
      }
    }
  `,
})
export class TodoListComponent {
  readonly todos = darshanQuery<Todo[]>('todos', { where: { done: false } });
}
```

### Signal Return Value

| Signal       | Type                       | Description                               |
|--------------|----------------------------|-------------------------------------------|
| `data`       | `Signal<T \| undefined>`    | Query data, `undefined` while loading     |
| `isLoading`  | `Signal<boolean>`          | Active loading or reconnecting            |
| `error`      | `Signal<DarshJError \| null>`| Query error or `null`                    |
| `refetch`    | `() => void`               | Manually re-execute (discard cache)       |

Each signal is independent -- reading `data` does not trigger re-render when only `isLoading` changes.

### Options

```typescript
darshanQuery<Todo[]>('todos', { where: { done: false } }, {
  debounceMs: 200,        // Debounce rapid updates
  skipInitialFetch: true,  // Do not set isLoading on first subscribe
});
```

### Lifecycle

The subscription is automatically torn down when the component or service is destroyed via `DestroyRef`. No manual cleanup required.

## RxJS Observables

`darshanQuery$()` wraps the WebSocket subscription as an RxJS `Observable` with `shareReplay(1)` applied by default.

```typescript
import { Component } from '@angular/core';
import { darshanQuery$ } from '@darshjdb/angular';

@Component({
  template: `
    <ul>
      <li *ngFor="let todo of todos$ | async">{{ todo.title }}</li>
    </ul>
  `,
})
export class TodoListComponent {
  readonly todos$ = darshanQuery$<Todo[]>('todos', { where: { done: false } })
    .pipe(map(result => result.data));
}
```

### shareReplay Behavior

- Late subscribers get the last emitted value immediately.
- The server subscription is torn down when all subscribers unsubscribe (`refCount: true`).
- No memory leak from unbounded replay buffers (`bufferSize: 1`).

### One-Shot Queries

For queries that should execute once and complete:

```typescript
import { darshanQueryOnce$ } from '@darshjdb/angular';

readonly userCount$ = darshanQueryOnce$<number>('users', { count: true });
```

### Observable Mutations

```typescript
import { darshanMutate$ } from '@darshjdb/angular';

this.addTodo$.pipe(
  switchMap(title =>
    darshanMutate$<Todo>('todos', { insert: { title, done: false } })
  ),
).subscribe(todo => console.log('Created:', todo.id));
```

### Error Handling

Observable queries deliver errors as emissions (not thrown), enabling `switchMap`/`catchError` composition:

```typescript
readonly data$ = darshanQuery$<Todo[]>('todos', {}).pipe(
  map(result => {
    if (result.error) throw result.error;
    return result.data;
  }),
  catchError(err => of([])),
);
```

## AuthGuard

Protect routes with `darshanAuthGuard`. Unauthenticated users are redirected to `/auth/sign-in` with a `returnUrl` query parameter.

```typescript
// app.routes.ts
import { darshanAuthGuard } from '@darshjdb/angular';

export const routes: Routes = [
  {
    path: 'dashboard',
    canActivate: [darshanAuthGuard],
    loadComponent: () => import('./dashboard.component'),
  },
  {
    path: 'admin',
    canActivate: [darshanAuthGuard],
    data: { authRedirect: '/login' },  // Custom redirect
    component: AdminComponent,
  },
];
```

### Role-Based Guard

```typescript
import { darshanRoleGuard } from '@darshjdb/angular';

{
  path: 'admin',
  canActivate: [darshanRoleGuard('admin')],
  component: AdminComponent,
}

{
  path: 'reports',
  canActivate: [darshanRoleGuard('admin', 'analyst')],
  component: ReportsComponent,
}
```

Users without all required roles are redirected to `/auth/unauthorized` (configurable via `route.data.unauthorizedRedirect`).

## HTTP Interceptor

Attach the DarshJDB JWT to outgoing requests automatically. The token is only added to requests whose URL starts with the configured `serverUrl`, preventing token leakage to third-party APIs.

```typescript
// main.ts
import { provideHttpClient, withInterceptors } from '@angular/common/http';
import { darshanAuthInterceptor } from '@darshjdb/angular';

bootstrapApplication(AppComponent, {
  providers: [
    provideDarshan({ serverUrl: 'https://db.example.com', appId: 'my-app' }),
    provideHttpClient(withInterceptors([darshanAuthInterceptor])),
  ],
});
```

## SSR with TransferState

For Angular Universal / SSR applications, queries executed on the server should be stored in `TransferState` and hydrated on the client. The SDK provides SSR utilities via the `ssr` module:

```typescript
// In a server-side resolver or guard:
import { inject } from '@angular/core';
import { DDB_CLIENT } from '@darshjdb/angular';

const client = inject(DDB_CLIENT);
const data = await client.query('users', { where: { active: true } });
// Store in TransferState for client-side hydration
```

The `darshanQuery()` signal and `darshanQuery$()` observable both work in SSR contexts -- they will wait for the first emission before completing server-side rendering when used with Angular's SSR serialization.
