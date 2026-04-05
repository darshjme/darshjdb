/**
 * Comprehensive tests for @darshan/react hooks and provider.
 *
 * These tests cover:
 *  - DarshanProvider context creation
 *  - useDarshanClient error when outside provider
 *  - useQuery subscription/unsubscription lifecycle
 *  - useMutation loading/error state
 *  - useAuth state changes and action callbacks
 *  - usePresence join/leave lifecycle
 *
 * All tests use a mock DarshanClientInterface to isolate React binding logic
 * from the actual client-core implementation.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import React, { createElement } from 'react';
import { renderHook, act, cleanup } from '@testing-library/react';

import { DarshanProvider, useDarshanClient } from '../provider';
import { useQuery } from '../use-query';
import { useMutation } from '../use-mutation';
import { useAuth } from '../use-auth';
import { usePresence } from '../use-presence';
import type {
  DarshanClientInterface,
  AuthState,
  AuthUser,
  PresencePeer,
  QuerySnapshot,
  Unsubscribe,
} from '../types';

/* ========================================================================== */
/*  Mock Client Factory                                                       */
/* ========================================================================== */

function createMockClient(overrides: Partial<DarshanClientInterface> = {}): DarshanClientInterface {
  const authListeners = new Set<(state: AuthState) => void>();
  const presenceListeners = new Map<string, Set<(peers: ReadonlyArray<PresencePeer>) => void>>();
  const querySubscriptions = new Map<string, (snap: QuerySnapshot<unknown>) => void>();
  let subCounter = 0;

  const defaultAuthState: AuthState = { user: null, isLoading: false };

  return {
    connect: vi.fn().mockResolvedValue(undefined),
    disconnect: vi.fn(),

    subscribe: vi.fn(<T>(_query: unknown, callback: (snap: QuerySnapshot<T>) => void): Unsubscribe => {
      const id = `sub_${++subCounter}`;
      querySubscriptions.set(id, callback as (snap: QuerySnapshot<unknown>) => void);
      return () => { querySubscriptions.delete(id); };
    }),
    query: vi.fn().mockResolvedValue([]),

    mutate: vi.fn().mockResolvedValue(undefined),

    signIn: vi.fn().mockResolvedValue({ id: 'u1', email: 'test@test.com', displayName: null, photoUrl: null, metadata: {} }),
    signUp: vi.fn().mockResolvedValue({ id: 'u2', email: 'new@test.com', displayName: null, photoUrl: null, metadata: {} }),
    signOut: vi.fn().mockResolvedValue(undefined),
    getAuthState: vi.fn(() => defaultAuthState),
    onAuthStateChange: vi.fn((cb: (state: AuthState) => void): Unsubscribe => {
      authListeners.add(cb);
      return () => { authListeners.delete(cb); };
    }),

    joinRoom: vi.fn().mockResolvedValue(undefined),
    leaveRoom: vi.fn().mockResolvedValue(undefined),
    publishPresence: vi.fn(),
    onPresenceChange: vi.fn(<S>(roomId: string, cb: (peers: ReadonlyArray<PresencePeer<S>>) => void): Unsubscribe => {
      if (!presenceListeners.has(roomId)) {
        presenceListeners.set(roomId, new Set());
      }
      presenceListeners.get(roomId)!.add(cb as (peers: ReadonlyArray<PresencePeer>) => void);
      return () => {
        presenceListeners.get(roomId)?.delete(cb as (peers: ReadonlyArray<PresencePeer>) => void);
      };
    }),

    upload: vi.fn().mockResolvedValue({ url: 'https://cdn.example.com/file.png', path: 'file.png', size: 1024, contentType: 'image/png' }),

    // Allow tests to inject overrides.
    ...overrides,

    // Expose internals for test assertions.
    __authListeners: authListeners,
    __presenceListeners: presenceListeners,
    __querySubscriptions: querySubscriptions,
  } as DarshanClientInterface & {
    __authListeners: Set<(state: AuthState) => void>;
    __presenceListeners: Map<string, Set<(peers: ReadonlyArray<PresencePeer>) => void>>;
    __querySubscriptions: Map<string, (snap: QuerySnapshot<unknown>) => void>;
  };
}

/* ========================================================================== */
/*  Provider wrapper factory                                                  */
/* ========================================================================== */

function makeWrapper(client: DarshanClientInterface) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return createElement(DarshanProvider, {
      serverUrl: 'https://db.test.com',
      appId: 'test',
      client,
      children,
    });
  };
}

/* ========================================================================== */
/*  DarshanProvider & useDarshanClient                                        */
/* ========================================================================== */

describe('DarshanProvider', () => {
  afterEach(() => {
    cleanup();
  });

  it('provides client via context', () => {
    const mockClient = createMockClient();
    const { result } = renderHook(() => useDarshanClient(), {
      wrapper: makeWrapper(mockClient),
    });

    expect(result.current).toBe(mockClient);
  });

  it('throws when useDarshanClient is used outside provider', () => {
    // Suppress React error output for this test
    const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    expect(() => {
      renderHook(() => useDarshanClient());
    }).toThrow('useDarshanClient must be used within a <DarshanProvider>');

    consoleSpy.mockRestore();
  });

  it('does not call connect/disconnect when external client is provided', () => {
    const mockClient = createMockClient();
    const { unmount } = renderHook(() => useDarshanClient(), {
      wrapper: makeWrapper(mockClient),
    });

    // External client means provider does NOT manage lifecycle
    expect(mockClient.connect).not.toHaveBeenCalled();

    unmount();
    expect(mockClient.disconnect).not.toHaveBeenCalled();
  });
});

/* ========================================================================== */
/*  useQuery                                                                  */
/* ========================================================================== */

describe('useQuery', () => {
  afterEach(() => {
    cleanup();
  });

  it('starts in loading state with empty data', () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useQuery({ collection: 'todos' }),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(result.current.isLoading).toBe(true);
    expect(result.current.data).toEqual([]);
    expect(result.current.error).toBeNull();
  });

  it('calls client.subscribe with the query', () => {
    const mockClient = createMockClient();
    renderHook(
      () => useQuery({ collection: 'todos' }),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(mockClient.subscribe).toHaveBeenCalled();
  });

  it('unsubscribes on unmount', () => {
    const unsub = vi.fn();
    const mockClient = createMockClient({
      subscribe: vi.fn(() => unsub),
    });

    const { unmount } = renderHook(
      () => useQuery({ collection: 'todos' }),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(unsub).not.toHaveBeenCalled();
    unmount();
    expect(unsub).toHaveBeenCalled();
  });

  it('does not subscribe when enabled is false', () => {
    const mockClient = createMockClient();
    renderHook(
      () => useQuery({ collection: 'todos' }, { enabled: false }),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(mockClient.subscribe).not.toHaveBeenCalled();
  });

  it('updates data when subscription callback fires', async () => {
    let subscribeCallback: ((snap: QuerySnapshot<unknown>) => void) | null = null;

    const mockClient = createMockClient({
      subscribe: vi.fn((_query: unknown, cb: (snap: QuerySnapshot<unknown>) => void) => {
        subscribeCallback = cb;
        return () => {};
      }),
    });

    const { result } = renderHook(
      () => useQuery<{ id: string; title: string }>({ collection: 'todos' }),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(result.current.isLoading).toBe(true);

    // Simulate server pushing data
    act(() => {
      subscribeCallback!({
        data: [{ id: '1', title: 'Buy milk' }],
        error: null,
      });
    });

    expect(result.current.isLoading).toBe(false);
    expect(result.current.data).toEqual([{ id: '1', title: 'Buy milk' }]);
    expect(result.current.error).toBeNull();
  });

  it('reports errors from subscription', () => {
    let subscribeCallback: ((snap: QuerySnapshot<unknown>) => void) | null = null;

    const mockClient = createMockClient({
      subscribe: vi.fn((_query: unknown, cb: (snap: QuerySnapshot<unknown>) => void) => {
        subscribeCallback = cb;
        return () => {};
      }),
    });

    const { result } = renderHook(
      () => useQuery({ collection: 'todos' }),
      { wrapper: makeWrapper(mockClient) },
    );

    act(() => {
      subscribeCallback!({
        data: [],
        error: new Error('Query failed'),
      });
    });

    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeInstanceOf(Error);
    expect(result.current.error!.message).toBe('Query failed');
  });

  it('resubscribes when query changes', () => {
    const unsub = vi.fn();
    let subscribeCount = 0;
    const mockClient = createMockClient({
      subscribe: vi.fn(() => {
        subscribeCount++;
        return unsub;
      }),
    });

    const { rerender } = renderHook(
      ({ collection }: { collection: string }) => useQuery({ collection }),
      {
        wrapper: makeWrapper(mockClient),
        initialProps: { collection: 'todos' },
      },
    );

    const initialCount = subscribeCount;
    rerender({ collection: 'posts' });

    // The previous subscription should have been torn down and a new one created.
    expect(unsub).toHaveBeenCalled();
    expect(subscribeCount).toBeGreaterThan(initialCount);
  });
});

/* ========================================================================== */
/*  useMutation                                                               */
/* ========================================================================== */

describe('useMutation', () => {
  afterEach(() => {
    cleanup();
  });

  it('starts with isLoading false and no error', () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useMutation(),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
    expect(typeof result.current.mutate).toBe('function');
  });

  it('calls client.mutate and resolves', async () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useMutation(),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      await result.current.mutate({
        type: 'insert',
        collection: 'todos',
        data: { title: 'New todo' },
      });
    });

    expect(mockClient.mutate).toHaveBeenCalledWith({
      type: 'insert',
      collection: 'todos',
      data: { title: 'New todo' },
    });
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('sets error state when mutation fails', async () => {
    const mockClient = createMockClient({
      mutate: vi.fn().mockRejectedValue(new Error('Write rejected')),
    });

    const { result } = renderHook(
      () => useMutation(),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      try {
        await result.current.mutate({
          type: 'insert',
          collection: 'todos',
          data: { title: 'Will fail' },
        });
      } catch {
        // Expected
      }
    });

    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeInstanceOf(Error);
    expect(result.current.error!.message).toBe('Write rejected');
  });

  it('mutate function has stable identity across renders', () => {
    const mockClient = createMockClient();
    const { result, rerender } = renderHook(
      () => useMutation(),
      { wrapper: makeWrapper(mockClient) },
    );

    const firstMutate = result.current.mutate;
    rerender();
    expect(result.current.mutate).toBe(firstMutate);
  });

  it('handles array of operations', async () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useMutation(),
      { wrapper: makeWrapper(mockClient) },
    );

    const ops = [
      { type: 'insert' as const, collection: 'todos', data: { title: 'A' } },
      { type: 'delete' as const, collection: 'todos', id: 'old-1' },
    ];

    await act(async () => {
      await result.current.mutate(ops);
    });

    expect(mockClient.mutate).toHaveBeenCalledWith(ops);
  });
});

/* ========================================================================== */
/*  useAuth                                                                   */
/* ========================================================================== */

describe('useAuth', () => {
  afterEach(() => {
    cleanup();
  });

  it('returns initial auth state from client', () => {
    const mockClient = createMockClient({
      getAuthState: vi.fn(() => ({ user: null, isLoading: false })),
    });

    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(result.current.user).toBeNull();
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it('subscribes to auth state changes on mount', () => {
    const mockClient = createMockClient();
    renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(mockClient.onAuthStateChange).toHaveBeenCalled();
  });

  it('updates when auth state changes', () => {
    let authCallback: ((state: AuthState) => void) | null = null;

    const mockClient = createMockClient({
      onAuthStateChange: vi.fn((cb: (state: AuthState) => void) => {
        authCallback = cb;
        return () => {};
      }),
    });

    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    const mockUser: AuthUser = {
      id: 'u1',
      email: 'alice@example.com',
      displayName: 'Alice',
      photoUrl: null,
      metadata: {},
    };

    act(() => {
      authCallback!({ user: mockUser, isLoading: false });
    });

    expect(result.current.user).toEqual(mockUser);
    expect(result.current.isLoading).toBe(false);
  });

  it('signIn calls client.signIn and returns user', async () => {
    const expectedUser: AuthUser = {
      id: 'u1',
      email: 'test@test.com',
      displayName: null,
      photoUrl: null,
      metadata: {},
    };

    const mockClient = createMockClient({
      signIn: vi.fn().mockResolvedValue(expectedUser),
    });

    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    let returnedUser: AuthUser | undefined;
    await act(async () => {
      returnedUser = await result.current.signIn({
        email: 'test@test.com',
        password: 'password',
      });
    });

    expect(returnedUser).toEqual(expectedUser);
    expect(mockClient.signIn).toHaveBeenCalledWith({
      email: 'test@test.com',
      password: 'password',
    });
  });

  it('signUp calls client.signUp with displayName', async () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      await result.current.signUp({
        email: 'new@test.com',
        password: 'pass123',
        displayName: 'New User',
      });
    });

    expect(mockClient.signUp).toHaveBeenCalledWith({
      email: 'new@test.com',
      password: 'pass123',
      displayName: 'New User',
    });
  });

  it('signOut calls client.signOut', async () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      await result.current.signOut();
    });

    expect(mockClient.signOut).toHaveBeenCalled();
  });

  it('sets error state when signIn fails', async () => {
    const mockClient = createMockClient({
      signIn: vi.fn().mockRejectedValue(new Error('Invalid credentials')),
    });

    const { result } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      try {
        await result.current.signIn({ email: 'a@b.com', password: 'wrong' });
      } catch {
        // Expected
      }
    });

    expect(result.current.error).toBeInstanceOf(Error);
    expect(result.current.error!.message).toBe('Invalid credentials');
  });

  it('action callbacks have stable identity', () => {
    const mockClient = createMockClient();
    const { result, rerender } = renderHook(
      () => useAuth(),
      { wrapper: makeWrapper(mockClient) },
    );

    const first = {
      signIn: result.current.signIn,
      signUp: result.current.signUp,
      signOut: result.current.signOut,
    };

    rerender();

    expect(result.current.signIn).toBe(first.signIn);
    expect(result.current.signUp).toBe(first.signUp);
    expect(result.current.signOut).toBe(first.signOut);
  });
});

/* ========================================================================== */
/*  usePresence                                                               */
/* ========================================================================== */

describe('usePresence', () => {
  afterEach(() => {
    cleanup();
  });

  it('joins the room on mount', async () => {
    const mockClient = createMockClient();
    renderHook(
      () => usePresence('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    // joinRoom is async, give it a tick
    await act(async () => {
      await Promise.resolve();
    });

    expect(mockClient.joinRoom).toHaveBeenCalledWith('room-1');
  });

  it('leaves the room on unmount', async () => {
    const mockClient = createMockClient();
    const { unmount } = renderHook(
      () => usePresence('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      await Promise.resolve();
    });

    unmount();

    expect(mockClient.leaveRoom).toHaveBeenCalledWith('room-1');
  });

  it('starts with empty peers', () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => usePresence('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    expect(result.current.peers).toEqual([]);
  });

  it('subscribes to presence changes after joining', async () => {
    const mockClient = createMockClient();
    renderHook(
      () => usePresence('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    await act(async () => {
      // Wait for joinRoom promise to resolve
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mockClient.onPresenceChange).toHaveBeenCalledWith(
      'room-1',
      expect.any(Function),
    );
  });

  it('publishState calls client.publishPresence', async () => {
    const mockClient = createMockClient();
    const { result } = renderHook(
      () => usePresence<{ x: number; y: number }>('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    act(() => {
      result.current.publishState({ x: 100, y: 200 });
    });

    expect(mockClient.publishPresence).toHaveBeenCalledWith(
      'room-1',
      { x: 100, y: 200 },
    );
  });

  it('updates peers when presence callback fires', async () => {
    let presenceCallback: ((peers: ReadonlyArray<PresencePeer>) => void) | null = null;

    const mockClient = createMockClient({
      onPresenceChange: vi.fn((_roomId: string, cb: (peers: ReadonlyArray<PresencePeer>) => void) => {
        presenceCallback = cb;
        return () => {};
      }),
    });

    const { result } = renderHook(
      () => usePresence<{ name: string }>('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    // Let joinRoom resolve
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    const mockPeers: PresencePeer<{ name: string }>[] = [
      { peerId: 'p1', state: { name: 'Alice' }, lastSeen: Date.now() },
      { peerId: 'p2', state: { name: 'Bob' }, lastSeen: Date.now() },
    ];

    act(() => {
      presenceCallback!(mockPeers);
    });

    expect(result.current.peers).toEqual(mockPeers);
    expect(result.current.peers).toHaveLength(2);
  });

  it('re-joins when roomId changes', async () => {
    const mockClient = createMockClient();
    const { rerender } = renderHook(
      ({ roomId }: { roomId: string }) => usePresence(roomId),
      {
        wrapper: makeWrapper(mockClient),
        initialProps: { roomId: 'room-1' },
      },
    );

    await act(async () => {
      await Promise.resolve();
    });

    rerender({ roomId: 'room-2' });

    await act(async () => {
      await Promise.resolve();
    });

    expect(mockClient.leaveRoom).toHaveBeenCalledWith('room-1');
    expect(mockClient.joinRoom).toHaveBeenCalledWith('room-2');
  });

  it('publishState has stable identity for same roomId', () => {
    const mockClient = createMockClient();
    const { result, rerender } = renderHook(
      () => usePresence('room-1'),
      { wrapper: makeWrapper(mockClient) },
    );

    const first = result.current.publishState;
    rerender();
    expect(result.current.publishState).toBe(first);
  });
});
