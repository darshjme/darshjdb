/**
 * @module presence
 * @description Presence tracking utilities for Angular applications.
 *
 * While the core presence injection is provided by `injectDarshanPresence()`
 * in the `inject` module, this module provides higher-level utilities:
 *
 * - `DarshanPresenceDirective` — A structural directive that auto-joins
 *   a presence room and exposes the state in the template context.
 * - `presenceUserCount()` — A signal that tracks just the user count
 *   for a room, useful for badges and indicators.
 *
 * @example
 * ```typescript
 * // Using the directive:
 * <div *darshanPresence="'room-123'; let users = users; let count = count">
 *   <span>{{ count }} online</span>
 *   <avatar *ngFor="let u of users" [user]="u" />
 * </div>
 *
 * // Using the signal helper:
 * readonly onlineCount = presenceUserCount('room-123');
 * ```
 */

import {
  inject,
  signal,
  DestroyRef,
  Directive,
  Input,
  TemplateRef,
  ViewContainerRef,
  type OnInit,
  type OnDestroy,
  type Signal,
} from '@angular/core';

import { DARSHAN_CLIENT, type PresenceHandle } from './tokens';
import type { PresenceUser } from './types';

// ── presenceUserCount ──────────────────────────────────────────────

/**
 * Lightweight signal that tracks only the user count for a presence room.
 *
 * Unlike `injectDarshanPresence()` which exposes full user data, this
 * helper is optimized for UI elements that only need a count (e.g.,
 * online badges, "X users viewing" indicators).
 *
 * @param roomId - The presence room to track.
 * @returns A read-only `Signal<number>` reflecting the current user count.
 *
 * @example
 * ```typescript
 * @Component({
 *   template: `<span class="badge">{{ onlineCount() }}</span>`,
 * })
 * export class OnlineBadge {
 *   readonly onlineCount = presenceUserCount('global-lobby');
 * }
 * ```
 */
export function presenceUserCount(roomId: string): Signal<number> {
  const client = inject(DARSHAN_CLIENT);
  const destroyRef = inject(DestroyRef);

  const _count = signal(0);

  const handle = client.joinPresence(
    roomId,
    {},
    (state) => {
      _count.set(state.count);
    },
  );

  destroyRef.onDestroy(() => handle.leave());

  return _count.asReadonly();
}

// ── DarshanPresenceDirective ───────────────────────────────────────

/**
 * Template context exposed by `*darshanPresence`.
 *
 * @internal
 */
export interface DarshanPresenceContext<TData = Record<string, unknown>> {
  /** Implicit value: the full users array. */
  $implicit: readonly PresenceUser<TData>[];
  /** All users currently present. */
  users: readonly PresenceUser<TData>[];
  /** Number of connected users. */
  count: number;
  /** The current user's own entry. */
  self: PresenceUser<TData> | null;
  /** Whether the presence channel is still connecting. */
  isLoading: boolean;
}

/**
 * Structural directive that joins a DarshanDB presence room and
 * exposes the state via template variables.
 *
 * Automatically leaves the room when the directive is destroyed.
 *
 * @example
 * ```html
 * <ng-container *darshanPresence="'collab-room'; let users; let count = count; let loading = isLoading">
 *   <loading-spinner *ngIf="loading" />
 *   <p>{{ count }} users editing</p>
 *   <user-avatar *ngFor="let u of users" [userId]="u.userId" />
 * </ng-container>
 * ```
 */
@Directive({
  // eslint-disable-next-line @angular-eslint/directive-selector
  selector: '[darshanPresence]',
  standalone: true,
})
export class DarshanPresenceDirective<TData = Record<string, unknown>>
  implements OnInit, OnDestroy
{
  /**
   * The room ID to join. Bound via the directive's microsyntax:
   * `*darshanPresence="'room-id'"`.
   */
  @Input({ required: true })
  darshanPresence!: string;

  /**
   * Optional initial data to broadcast for the current user.
   * `*darshanPresence="'room'; data: myData"`.
   */
  @Input()
  darshanPresenceData: TData = {} as TData;

  private readonly _templateRef = inject(TemplateRef<DarshanPresenceContext<TData>>);
  private readonly _viewContainer = inject(ViewContainerRef);
  private readonly _client = inject(DARSHAN_CLIENT);

  private _handle: PresenceHandle<TData> | null = null;
  private _context: DarshanPresenceContext<TData> = {
    $implicit: [],
    users: [],
    count: 0,
    self: null,
    isLoading: true,
  };

  ngOnInit(): void {
    // Create the embedded view with the initial context.
    this._viewContainer.createEmbeddedView(this._templateRef, this._context);

    // Join the presence room.
    this._handle = this._client.joinPresence<TData>(
      this.darshanPresence,
      this.darshanPresenceData,
      (state) => {
        const currentUser = this._client.getUser();
        const selfEntry = currentUser
          ? (state.users.find((u) => u.userId === currentUser.id) ?? null)
          : null;

        // Mutate the context object in place so the embedded view
        // picks up changes on the next change detection cycle.
        this._context.$implicit = state.users;
        this._context.users = state.users;
        this._context.count = state.count;
        this._context.self = selfEntry;
        this._context.isLoading = false;
      },
    );
  }

  ngOnDestroy(): void {
    this._handle?.leave();
    this._handle = null;
  }

  /**
   * Static type guard for the template context.
   *
   * Enables strong typing in templates when using the directive
   * with `*ngTemplateOutlet` or structural directive microsyntax.
   *
   * @internal
   */
  static ngTemplateContextGuard<TData>(
    _dir: DarshanPresenceDirective<TData>,
    _ctx: unknown,
  ): _ctx is DarshanPresenceContext<TData> {
    return true;
  }
}
