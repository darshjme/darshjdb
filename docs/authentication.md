# Authentication

DarshanDB includes a complete auth system. No third-party services required.

## Email / Password

```typescript
// Sign up
await db.auth.signUp({ email: 'user@example.com', password: 'SecurePass123!' });

// Sign in
await db.auth.signIn({ email: 'user@example.com', password: 'SecurePass123!' });

// Sign out
await db.auth.signOut();

// Get current user
const user = db.auth.getUser();
```

Passwords are hashed with **Argon2id** (memory=64MB, iterations=3, parallelism=4). Account locks after 5 failed attempts for 30 minutes.

## Magic Links

```typescript
// Request magic link (sent via email)
await db.auth.sendMagicLink({ email: 'user@example.com' });

// Verify (from the link callback)
await db.auth.verifyMagicLink({ token: 'abc123...' });
```

## OAuth

```typescript
// Opens popup for OAuth flow
await db.auth.signInWithOAuth('google');
await db.auth.signInWithOAuth('github');
await db.auth.signInWithOAuth('apple');
await db.auth.signInWithOAuth('discord');
```

### Configuration

Set OAuth credentials via environment variables:

```bash
DARSHAN_OAUTH_GOOGLE_CLIENT_ID=...
DARSHAN_OAUTH_GOOGLE_CLIENT_SECRET=...
DARSHAN_OAUTH_GITHUB_CLIENT_ID=...
DARSHAN_OAUTH_GITHUB_CLIENT_SECRET=...
```

## Multi-Factor Authentication

### TOTP (Google Authenticator)

```typescript
// Enable MFA
const { secret, qrCodeUri } = await db.auth.enableMFA();
// Show QR code to user, then verify:
await db.auth.verifyMFA({ code: '123456' });
```

### Recovery Codes

When MFA is enabled, 10 one-time recovery codes are generated. Display them once — they cannot be retrieved again.

## Session Management

```typescript
// List active sessions
const sessions = await db.auth.listSessions();

// Revoke a specific session
await db.auth.revokeSession(sessionId);

// Revoke all other sessions
await db.auth.revokeAllSessions();
```

## Auth State Changes

```typescript
db.auth.onAuthStateChange((user) => {
  if (user) {
    console.log('Signed in:', user.email);
  } else {
    console.log('Signed out');
  }
});
```

## React Hook

```tsx
function AuthButton() {
  const { user, signIn, signOut, isLoading } = db.useAuth();

  if (isLoading) return <Spinner />;
  if (user) return <button onClick={signOut}>Sign Out ({user.email})</button>;
  return <button onClick={() => signIn({ email, password })}>Sign In</button>;
}
```
