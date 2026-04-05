# @darshan/angular

Angular SDK for DarshanDB -- Signals, Observables, SSR, and standalone component support.

## Install

```bash
npm install @darshan/angular
```

Requires Angular 16+ as a peer dependency. Angular 17+ recommended for Signals support.

## Setup

```typescript
// app.config.ts (standalone)
import { provideDarshan } from '@darshan/angular';

export const appConfig = {
  providers: [
    provideDarshan({ appId: 'my-app' }),
  ],
};
```

Or with NgModule:

```typescript
// app.module.ts
import { DarshanModule } from '@darshan/angular';

@NgModule({
  imports: [
    DarshanModule.forRoot({ appId: 'my-app' }),
  ],
})
export class AppModule {}
```

## Signals API (Angular 17+)

### Live Queries

```typescript
import { Component } from '@angular/core';
import { injectQuery } from '@darshan/angular';

@Component({
  selector: 'app-todos',
  template: `
    @if (todos.isLoading()) {
      <p>Loading...</p>
    } @else if (todos.error()) {
      <p>Error: {{ todos.error()?.message }}</p>
    } @else {
      <ul>
        @for (todo of todos.data()?.todos; track todo.id) {
          <li>{{ todo.title }}</li>
        }
      </ul>
    }
  `,
})
export class TodosComponent {
  todos = injectQuery({ todos: { $where: { done: false }, $order: { createdAt: 'desc' } } });
}
```

### Authentication

```typescript
import { Component, inject } from '@angular/core';
import { DarshanAuthService } from '@darshan/angular';

@Component({
  template: `
    @if (auth.user()) {
      <span>{{ auth.user()?.email }}</span>
      <button (click)="auth.signOut()">Sign Out</button>
    } @else {
      <button (click)="auth.signInWithOAuth('google')">Sign In with Google</button>
    }
  `
})
export class AuthComponent {
  auth = inject(DarshanAuthService);
}
```

### Mutations

```typescript
import { Component, inject } from '@angular/core';
import { DarshanService } from '@darshan/angular';

@Component({
  template: `
    <form (ngSubmit)="addTodo(titleInput.value)">
      <input #titleInput />
      <button type="submit">Add</button>
    </form>
  `
})
export class AddTodoComponent {
  private db = inject(DarshanService);

  async addTodo(title: string) {
    await this.db.transact(
      this.db.tx.todos[this.db.id()].set({ title, done: false, createdAt: Date.now() })
    );
  }
}
```

## RxJS Observables API

For traditional Angular patterns or Angular 16:

```typescript
import { Component, inject } from '@angular/core';
import { DarshanService } from '@darshan/angular';
import { AsyncPipe } from '@angular/common';

@Component({
  selector: 'app-todos',
  imports: [AsyncPipe],
  template: `
    <ul>
      <li *ngFor="let todo of (todos$ | async)?.todos">{{ todo.title }}</li>
    </ul>
  `,
})
export class TodosComponent {
  private db = inject(DarshanService);
  todos$ = this.db.query$({ todos: { $where: { done: false } } });
}
```

## Route Guards

Protect routes with the built-in auth guard:

```typescript
// app.routes.ts
import { darshanAuthGuard } from '@darshan/angular';

export const routes = [
  { path: 'dashboard', component: DashboardComponent, canActivate: [darshanAuthGuard] },
  { path: 'admin', component: AdminComponent, canActivate: [darshanAuthGuard({ role: 'admin' })] },
  { path: 'login', component: LoginComponent },
];
```

## Presence

```typescript
import { Component, inject } from '@angular/core';
import { injectPresence } from '@darshan/angular';

@Component({
  template: `
    @for (peer of presence.peers(); track peer.id) {
      <div class="cursor" [style.left.px]="peer.data.cursor?.x" [style.top.px]="peer.data.cursor?.y">
        {{ peer.data.name }}
      </div>
    }
  `
})
export class CursorsComponent {
  presence = injectPresence('doc-123', { name: 'User', cursor: null });

  onMouseMove(event: MouseEvent) {
    this.presence.update({ cursor: { x: event.clientX, y: event.clientY } });
  }
}
```

## SSR Support

Works with Angular Universal out of the box. Queries executed during SSR are serialized into the HTML and rehydrated on the client.

```typescript
// server.ts
import { provideDarshanServer } from '@darshan/angular/server';

const serverConfig = {
  providers: [
    provideDarshanServer({ serverUrl: 'http://localhost:7700', adminToken: '...' }),
  ],
};
```

## Features

- **Angular Signals** -- First-class support for Angular 17+ signals
- **RxJS integration** -- Observable-based API for traditional Angular patterns
- **Route Guards** -- Auth guards for protected routes (role-based support)
- **SSR support** -- Works with Angular Universal
- **Standalone components** -- No NgModule required
- **OnPush compatible** -- Works with `ChangeDetectionStrategy.OnPush`

## Building

```bash
npm run build      # Produces dist/ with ESM, CJS, and type declarations
npm run dev        # Watch mode
npm test           # Run tests
npm run typecheck  # Type check
```

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
- [Presence](../../docs/presence.md)
