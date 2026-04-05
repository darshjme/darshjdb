/**
 * Authentication client for DarshanDB.
 *
 * Supports email/password sign-up and sign-in, OAuth popup flows,
 * automatic token refresh, and pluggable token storage.
 *
 * @module auth
 */

import type { DarshanDB } from './client.js';
import type {
  User,
  AuthTokens,
  OAuthProvider,
  AuthStateEvent,
  AuthStateCallback,
  TokenStorage,
  Unsubscribe,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

const TOKEN_KEY_ACCESS = 'darshan_access_token';
const TOKEN_KEY_REFRESH = 'darshan_refresh_token';
const TOKEN_KEY_EXPIRES = 'darshan_token_expires';
/** Refresh the token 60 seconds before expiry. */
const REFRESH_BUFFER_MS = 60_000;

/* -------------------------------------------------------------------------- */
/*  Default localStorage adapter                                              */
/* -------------------------------------------------------------------------- */

/**
 * Default token storage backed by `localStorage`.
 * Falls back to an in-memory map in non-browser environments.
 */
class LocalStorageAdapter implements TokenStorage {
  private _privateFallback = new Map<string, string>();
  private _privateHasLocalStorage: boolean;

  constructor() {
    try {
      // Feature-detect localStorage availability.
      localStorage.setItem('__darshan_test__', '1');
      localStorage.removeItem('__darshan_test__');
      this._privateHasLocalStorage = true;
    } catch {
      this._privateHasLocalStorage = false;
    }
  }

  get(key: string): string | null {
    if (this._privateHasLocalStorage) {
      return localStorage.getItem(key);
    }
    return this._privateFallback.get(key) ?? null;
  }

  set(key: string, value: string): void {
    if (this._privateHasLocalStorage) {
      localStorage.setItem(key, value);
    } else {
      this._privateFallback.set(key, value);
    }
  }

  remove(key: string): void {
    if (this._privateHasLocalStorage) {
      localStorage.removeItem(key);
    } else {
      this._privateFallback.delete(key);
    }
  }
}

/* -------------------------------------------------------------------------- */
/*  AuthClient                                                                */
/* -------------------------------------------------------------------------- */

/**
 * Authentication client providing sign-up, sign-in, OAuth, token management,
 * and auth state change notifications.
 *
 * @example
 * ```ts
 * const auth = new AuthClient(db);
 *
 * auth.onAuthStateChange(({ user }) => {
 *   console.log('Auth state:', user);
 * });
 *
 * await auth.signIn({ email: 'alice@example.com', password: 's3cret' });
 * ```
 */
export class AuthClient {
  private _privateClient: DarshanDB;
  private _privateStorage: TokenStorage;
  private _privateUser: User | null = null;
  private _privateTokens: AuthTokens | null = null;
  private _privateListeners = new Set<AuthStateCallback>();
  private _privateRefreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(client: DarshanDB, storage?: TokenStorage) {
    this._privateClient = client;
    this._privateStorage = storage ?? new LocalStorageAdapter();
  }

  /* -- Initialisation ----------------------------------------------------- */

  /**
   * Restore a persisted session from token storage and set up auto-refresh.
   * Call once after creating the AuthClient.
   */
  async init(): Promise<void> {
    const accessToken = await this._privateStorage.get(TOKEN_KEY_ACCESS);
    const refreshToken = await this._privateStorage.get(TOKEN_KEY_REFRESH);
    const expiresStr = await this._privateStorage.get(TOKEN_KEY_EXPIRES);

    if (accessToken && refreshToken && expiresStr) {
      const expiresAt = parseInt(expiresStr, 10);
      this._privateTokens = { accessToken, refreshToken, expiresAt };

      if (Date.now() >= expiresAt - REFRESH_BUFFER_MS) {
        // Token expired or about to; refresh immediately.
        try {
          await this._privateRefreshTokens();
        } catch {
          // Refresh failed — clear session.
          await this._privateClearSession();
          return;
        }
      } else {
        this._privateClient.setAuthToken(accessToken);
        this._privateScheduleRefresh(expiresAt);
      }

      // Fetch current user profile.
      try {
        await this._privateFetchUser();
      } catch {
        await this._privateClearSession();
      }
    }
  }

  /* -- Sign Up ------------------------------------------------------------ */

  /**
   * Create a new account with email and password.
   *
   * @param params - Email and password.
   * @returns The newly created user.
   */
  async signUp(params: {
    email: string;
    password: string;
    displayName?: string;
  }): Promise<User> {
    const resp = await fetch(
      this._privateClient.getRestUrl('/auth/signup'),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(params),
      },
    );

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`Sign-up failed (${resp.status}): ${body}`);
    }

    const data = (await resp.json()) as { user: User; tokens: AuthTokens };
    await this._privateSetSession(data.user, data.tokens);
    return data.user;
  }

  /* -- Sign In ------------------------------------------------------------ */

  /**
   * Sign in with email and password.
   *
   * @param params - Email and password.
   * @returns The authenticated user.
   */
  async signIn(params: { email: string; password: string }): Promise<User> {
    const resp = await fetch(
      this._privateClient.getRestUrl('/auth/signin'),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(params),
      },
    );

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`Sign-in failed (${resp.status}): ${body}`);
    }

    const data = (await resp.json()) as { user: User; tokens: AuthTokens };
    await this._privateSetSession(data.user, data.tokens);
    return data.user;
  }

  /* -- OAuth -------------------------------------------------------------- */

  /**
   * Sign in using an OAuth provider via a popup window.
   *
   * Opens the provider's authorization URL in a popup, listens for the
   * callback, and exchanges the code for tokens.
   *
   * @param provider - The OAuth provider identifier.
   * @returns The authenticated user.
   */
  async signInWithOAuth(provider: OAuthProvider): Promise<User> {
    const authUrl = this._privateClient.getRestUrl(
      `/auth/oauth/${provider}/authorize`,
    );

    return new Promise<User>((resolve, reject) => {
      const width = 500;
      const height = 600;
      const left = window.screenX + (window.innerWidth - width) / 2;
      const top = window.screenY + (window.innerHeight - height) / 2;

      const popup = window.open(
        authUrl,
        'ddb-oauth',
        `width=${width},height=${height},left=${left},top=${top},popup=yes`,
      );

      if (!popup) {
        reject(new Error('Popup blocked. Please allow popups for this site.'));
        return;
      }

      const handleMessage = async (event: MessageEvent) => {
        // Validate origin.
        if (event.origin !== new URL(this._privateClient.serverUrl).origin) {
          return;
        }

        const data = event.data as {
          type?: string;
          user?: User;
          tokens?: AuthTokens;
          error?: string;
        };

        if (data.type !== 'ddb-oauth-callback') return;

        window.removeEventListener('message', handleMessage);
        popup.close();

        if (data.error) {
          reject(new Error(`OAuth failed: ${data.error}`));
          return;
        }

        if (data.user && data.tokens) {
          await this._privateSetSession(data.user, data.tokens);
          resolve(data.user);
        } else {
          reject(new Error('OAuth callback missing user or tokens'));
        }
      };

      window.addEventListener('message', handleMessage);

      // Poll for popup close (user may close manually).
      const pollTimer = setInterval(() => {
        if (popup.closed) {
          clearInterval(pollTimer);
          window.removeEventListener('message', handleMessage);
          // Only reject if we haven't resolved yet.
          reject(new Error('OAuth popup was closed by the user'));
        }
      }, 500);
    });
  }

  /* -- Sign Out ----------------------------------------------------------- */

  /**
   * Sign out the current user and clear all stored tokens.
   */
  async signOut(): Promise<void> {
    if (this._privateTokens) {
      try {
        await fetch(this._privateClient.getRestUrl('/auth/signout'), {
          method: 'POST',
          headers: {
            Authorization: `Bearer ${this._privateTokens.accessToken}`,
          },
        });
      } catch {
        /* best effort */
      }
    }

    await this._privateClearSession();
  }

  /* -- Accessors ---------------------------------------------------------- */

  /**
   * Get the currently authenticated user, or `null` if not signed in.
   */
  getUser(): User | null {
    return this._privateUser;
  }

  /**
   * Get the current auth tokens, or `null` if not signed in.
   */
  getTokens(): AuthTokens | null {
    return this._privateTokens;
  }

  /**
   * Register a listener for auth state changes.
   *
   * The callback fires immediately with the current state, then on every
   * subsequent change (sign-in, sign-out, token refresh).
   *
   * @returns An unsubscribe function.
   */
  onAuthStateChange(callback: AuthStateCallback): Unsubscribe {
    this._privateListeners.add(callback);

    // Deliver current state immediately.
    try {
      callback({ user: this._privateUser, tokens: this._privateTokens });
    } catch {
      /* listener error */
    }

    return () => {
      this._privateListeners.delete(callback);
    };
  }

  /* -- Internal ----------------------------------------------------------- */

  private async _privateSetSession(
    user: User,
    tokens: AuthTokens,
  ): Promise<void> {
    this._privateUser = user;
    this._privateTokens = tokens;

    await this._privateStorage.set(TOKEN_KEY_ACCESS, tokens.accessToken);
    await this._privateStorage.set(TOKEN_KEY_REFRESH, tokens.refreshToken);
    await this._privateStorage.set(
      TOKEN_KEY_EXPIRES,
      tokens.expiresAt.toString(),
    );

    this._privateClient.setAuthToken(tokens.accessToken);
    this._privateScheduleRefresh(tokens.expiresAt);
    this._privateNotify();
  }

  private async _privateClearSession(): Promise<void> {
    this._privateUser = null;
    this._privateTokens = null;

    if (this._privateRefreshTimer) {
      clearTimeout(this._privateRefreshTimer);
      this._privateRefreshTimer = null;
    }

    await this._privateStorage.remove(TOKEN_KEY_ACCESS);
    await this._privateStorage.remove(TOKEN_KEY_REFRESH);
    await this._privateStorage.remove(TOKEN_KEY_EXPIRES);

    this._privateClient.setAuthToken(null);
    this._privateNotify();
  }

  private async _privateRefreshTokens(): Promise<void> {
    if (!this._privateTokens?.refreshToken) {
      throw new Error('No refresh token available');
    }

    const resp = await fetch(
      this._privateClient.getRestUrl('/auth/refresh'),
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          refreshToken: this._privateTokens.refreshToken,
        }),
      },
    );

    if (!resp.ok) {
      throw new Error(`Token refresh failed (${resp.status})`);
    }

    const data = (await resp.json()) as { tokens: AuthTokens };
    this._privateTokens = data.tokens;

    await this._privateStorage.set(TOKEN_KEY_ACCESS, data.tokens.accessToken);
    await this._privateStorage.set(TOKEN_KEY_REFRESH, data.tokens.refreshToken);
    await this._privateStorage.set(
      TOKEN_KEY_EXPIRES,
      data.tokens.expiresAt.toString(),
    );

    this._privateClient.setAuthToken(data.tokens.accessToken);
    this._privateScheduleRefresh(data.tokens.expiresAt);
  }

  private _privateScheduleRefresh(expiresAt: number): void {
    if (this._privateRefreshTimer) {
      clearTimeout(this._privateRefreshTimer);
    }

    const delay = Math.max(0, expiresAt - Date.now() - REFRESH_BUFFER_MS);

    this._privateRefreshTimer = setTimeout(async () => {
      try {
        await this._privateRefreshTokens();
      } catch {
        console.warn('[DarshanDB Auth] Auto-refresh failed; clearing session.');
        await this._privateClearSession();
      }
    }, delay);
  }

  private async _privateFetchUser(): Promise<void> {
    if (!this._privateTokens) return;

    const resp = await fetch(this._privateClient.getRestUrl('/auth/me'), {
      headers: { Authorization: `Bearer ${this._privateTokens.accessToken}` },
    });

    if (!resp.ok) {
      throw new Error(`Failed to fetch user (${resp.status})`);
    }

    const data = (await resp.json()) as { user: User };
    this._privateUser = data.user;
    this._privateNotify();
  }

  private _privateNotify(): void {
    const event: AuthStateEvent = {
      user: this._privateUser,
      tokens: this._privateTokens,
    };
    for (const cb of this._privateListeners) {
      try {
        cb(event);
      } catch {
        /* listener errors must not break the notification loop */
      }
    }
  }
}
