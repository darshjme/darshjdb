import { describe, expect, it, vi } from 'vitest';
import { DestroyRef, Injector, PLATFORM_ID, TransferState, runInInjectionContext } from '@angular/core';
import { DDB_CLIENT } from './tokens';
import { darshanQuery$ } from './query.observable';
import { darshanQuery } from './query.signal';
import { darshanTransferQuery } from './ssr';
vi.mock('@angular/common', () => ({ isPlatformServer: (id: string) => id === 'server', isPlatformBrowser: (id: string) => id === 'browser' }));

describe('retired query state', () => {
  for (const create of [darshanQuery, darshanTransferQuery]) {
    it(`clears private rows in ${create.name}`, () => {
      let receive!: (value: { data: unknown[]; error: { code: string; message: string } | null }) => void;
      const injector = Injector.create({ providers: [
        { provide: DDB_CLIENT, useValue: { subscribe: (_collection: string, _query: unknown, callback: typeof receive) => { receive = callback; return () => {}; } } },
        { provide: DestroyRef, useValue: { onDestroy: () => {} } },
        { provide: TransferState, useValue: new TransferState() },
        { provide: PLATFORM_ID, useValue: 'browser' },
      ] });
      const state = runInInjectionContext(injector, () => create('notes', {}));
      receive({ data: [{ id: 'private' }], error: null });
      expect(state.data()).toEqual([{ id: 'private' }]);
      receive({ data: [], error: { code: 'SubscriptionEndedError', message: 'Credentials changed' } });
      expect(state.data()).toBeUndefined();
      expect(state.error()?.code).toBe('SubscriptionEndedError');
      injector.destroy();
    });
  }
  for (const observable of [false, true]) {
    it(`retires immediately despite debounce (${observable ? 'observable' : 'signal'})`, () => {
      vi.useFakeTimers();
      let receive!: (value: any) => void;
      const injector = Injector.create({ providers: [
        { provide: DDB_CLIENT, useValue: { subscribe: (_c: string, _q: unknown, callback: typeof receive) => { receive = callback; return () => {}; } } },
        { provide: DestroyRef, useValue: { onDestroy: () => {} } },
      ] });
      const values: any[] = [];
      const state = runInInjectionContext(injector, () => observable
        ? darshanQuery$('notes', {}, { debounceMs: 100 }).subscribe(value => values.push(value))
        : darshanQuery('notes', {}, { debounceMs: 100 }));
      receive({ data: [{ id: 'private' }], error: null });
      receive({ data: [], error: { code: 'SubscriptionEndedError', message: 'Credentials changed' } });
      if (observable) expect(values).toEqual([{ data: [], error: { code: 'SubscriptionEndedError', message: 'Credentials changed', status: undefined } }]);
      else { expect((state as any).data()).toBeUndefined(); expect((state as any).error()?.code).toBe('SubscriptionEndedError'); }
      vi.advanceTimersByTime(200);
      if (observable) { expect(values).toHaveLength(1); (state as any).unsubscribe(); }
      else expect((state as any).data()).toBeUndefined();
      injector.destroy();
      vi.useRealTimers();
    });
  }

});
