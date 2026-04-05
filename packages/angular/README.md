# @darshan/angular

Angular SDK for DarshanDB -- Signals, Observables, SSR, and standalone component support.

## Install

```bash
npm install @darshan/angular
```

## Usage

### Module Setup

```typescript
// app.config.ts
import { provideDarshan } from '@darshan/angular';

export const appConfig = {
  providers: [
    provideDarshan({ appId: 'my-app' }),
  ],
};
```

### Signals (Angular 17+)

```typescript
import { Component, inject } from '@angular/core';
import { DarshanService } from '@darshan/angular';

@Component({
  selector: 'app-todos',
  template: `
    @if (todos.isLoading()) {
      <p>Loading...</p>
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
  private db = inject(DarshanService);
  todos = this.db.query({ todos: { $where: { done: false } } });
}
```

### RxJS Observables

```typescript
import { DarshanService } from '@darshan/angular';

@Component({ /* ... */ })
export class TodosComponent {
  private db = inject(DarshanService);
  todos$ = this.db.query$({ todos: { $where: { done: false } } });
}
```

## Features

- **Angular Signals** -- First-class support for Angular 17+ signals
- **RxJS integration** -- Observable-based API for traditional Angular patterns
- **Route Guards** -- Auth guards for protected routes
- **SSR support** -- Works with Angular Universal
- **Standalone components** -- No NgModule required

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Query Language](../../docs/query-language.md)
- [Authentication](../../docs/authentication.md)
