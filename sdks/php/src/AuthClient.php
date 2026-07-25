<?php

declare(strict_types=1);

namespace Darshjdb;

/**
 * Authentication client for DarshJDB.
 *
 * Handles user registration, login, logout, and current-user retrieval.
 * Tokens are managed automatically on the parent {@see Client} instance.
 *
 * Usage:
 *   $auth = $db->auth();
 *   $user = $auth->signUp('alice@example.com', 'strongpassword', ['displayName' => 'Alice']);
 *   $user = $auth->signIn('alice@example.com', 'strongpassword');
 *   $me   = $auth->getUser();
 *   $auth->signOut();
 */
class AuthClient
{
    public function __construct(private readonly Client $client)
    {
    }

    /**
     * Register a new user with email and password.
     *
     * @param string               $email    User's email address.
     * @param string               $password User's password (min 8 characters recommended).
     * @param array<string, mixed> $profile  Optional profile fields (name).
     * @return array{user: array<string, mixed>, access_token: string, refresh_token: string}
     *
     * @throws Exception On validation or server errors.
     */
    public function signUp(string $email, string $password, array $profile = []): array
    {
        $result = $this->client->post('/api/auth/signup', array_merge([
            'email'    => $email,
            'password' => $password,
        ], $profile));

        $this->captureToken($result);

        return $result;
    }

    /**
     * Authenticate an existing user with email and password.
     *
     * @param string $email    User's email address.
     * @param string $password User's password.
     * @return array{user: array<string, mixed>, access_token: string, refresh_token: string}
     *
     * @throws Exception On invalid credentials or server errors.
     */
    public function signIn(string $email, string $password): array
    {
        $result = $this->client->post('/api/auth/signin', [
            'email'    => $email,
            'password' => $password,
        ]);

        $this->captureToken($result);

        return $result;
    }

    /**
     * Exchange an OAuth2 authorization code for a session.
     *
     * Called without a code, the server returns an authorize URL to redirect to.
     *
     * @param string $provider      OAuth provider name (google, github, apple, discord).
     * @param string $code          Authorization code from the provider callback.
     * @param string $state         State parameter from the provider callback.
     * @param string $pkceVerifier  PKCE verifier matching the challenge sent to the provider.
     * @return array{user: array<string, mixed>, access_token: string, refresh_token: string}
     *
     * @throws Exception On OAuth or server errors.
     */
    public function signInWithOAuth(
        string $provider,
        string $code = '',
        string $state = '',
        string $pkceVerifier = '',
    ): array {
        $result = $this->client->post("/api/auth/oauth/{$provider}", [
            'code'          => $code,
            'state'         => $state,
            'pkce_verifier' => $pkceVerifier,
        ]);

        $this->captureToken($result);

        return $result;
    }

    /**
     * Sign out the current user and invalidate their session.
     *
     * Clears the stored token on the client.
     *
     * @return array<string, mixed> Server acknowledgement.
     *
     * @throws Exception On server errors.
     */
    public function signOut(): array
    {
        $result = $this->client->post('/api/auth/signout');
        $this->client->setToken(null);

        return $result;
    }

    /**
     * Retrieve the currently authenticated user.
     *
     * @return array{id: string, email: string, displayName?: string, avatarUrl?: string, metadata?: array<string, mixed>}|null
     *   Returns null if no valid session exists.
     *
     * @throws Exception On server errors.
     */
    public function getUser(): ?array
    {
        try {
            return $this->client->get('/api/auth/me');
        } catch (Exception $e) {
            if ($e->getStatusCode() === 401) {
                return null;
            }
            throw $e;
        }
    }

    /**
     * Refresh the access token using a refresh token.
     *
     * @param string $refreshToken The refresh token from a previous sign-in.
     * @return array{access_token: string, refresh_token: string, expires_in: int, token_type: string}
     *
     * @throws Exception On invalid or expired refresh token.
     */
    public function refresh(string $refreshToken): array
    {
        $result = $this->client->post('/api/auth/refresh', [
            'refresh_token' => $refreshToken,
        ]);

        $this->captureToken($result);

        return $result;
    }

    /**
     * Manually set the access token on the client.
     *
     * Useful when restoring sessions from stored tokens.
     *
     * @param string $token A valid access token.
     */
    public function setToken(string $token): void
    {
        $this->client->setToken($token);
    }

    /**
     * Store the access token from an auth response on the parent client.
     *
     * The server emits snake_case (`access_token`); camelCase is accepted
     * for compatibility with older builds.
     *
     * @param array<string, mixed> $result A decoded auth response.
     */
    private function captureToken(array $result): void
    {
        $token = $result['access_token'] ?? $result['accessToken'] ?? null;

        if (is_string($token) && $token !== '') {
            $this->client->setToken($token);
        }
    }
}
