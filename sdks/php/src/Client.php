<?php

declare(strict_types=1);

namespace Darshjdb;

use GuzzleHttp\Client as HttpClient;
use GuzzleHttp\Exception\GuzzleException;
use GuzzleHttp\RequestOptions;

/**
 * Main DarshJDB client for PHP applications.
 *
 * Provides access to auth, querying, transactions, server-side functions,
 * and file storage through a unified interface.
 *
 * Usage:
 *   $db = new \Darshjdb\Client([
 *       'serverUrl' => 'https://db.example.com',
 *       'apiKey'    => 'your-api-key',
 *   ]);
 *
 *   $user = $db->auth()->signIn('user@example.com', 'password');
 *   $posts = $db->data('posts')->where('published', '=', true)->limit(10)->get();
 */
class Client
{
    private HttpClient $http;
    private string $serverUrl;
    private string $apiKey;
    private ?string $token = null;
    private ?AuthClient $authClient = null;
    private ?StorageClient $storageClient = null;

    /**
     * Create a new DarshJDB client.
     *
     * @param array{serverUrl: string, apiKey: string, timeout?: int} $config
     *   - serverUrl: Base URL of the DarshJDB server.
     *   - apiKey:    Application API key from the DarshJDB dashboard.
     *   - timeout:   HTTP request timeout in seconds (default: 30).
     */
    public function __construct(array $config)
    {
        if (empty($config['serverUrl'])) {
            throw new \InvalidArgumentException('serverUrl is required.');
        }
        if (empty($config['apiKey'])) {
            throw new \InvalidArgumentException('apiKey is required.');
        }

        $this->serverUrl = rtrim($config['serverUrl'], '/');
        $this->apiKey = $config['apiKey'];

        $this->http = new HttpClient([
            'base_uri' => $this->serverUrl,
            'timeout'  => $config['timeout'] ?? 30,
            'headers'  => [
                'Content-Type' => 'application/json',
                'Accept'       => 'application/json',
            ],
        ]);
    }

    /**
     * Get the authentication client.
     *
     * Provides methods for sign-up, sign-in, sign-out, and user retrieval.
     */
    public function auth(): AuthClient
    {
        if ($this->authClient === null) {
            $this->authClient = new AuthClient($this);
        }

        return $this->authClient;
    }

    /**
     * Execute a raw DarshanQL query.
     *
     * @param array<string, mixed> $query The DarshanQL query descriptor.
     * @return array{data: array<int, array<string, mixed>>, txId: string}
     *
     * @throws Exception On server or network errors.
     */
    public function query(array $query): array
    {
        return $this->post('/api/query', $query);
    }

    /**
     * Start a fluent query builder for a given entity (collection).
     *
     * Usage:
     *   $db->data('users')->where('age', '>=', 18)->orderBy('name')->limit(25)->get();
     *   $db->data('posts')->create(['title' => 'Hello', 'body' => '...']);
     *
     * @param string $entity The collection/entity name.
     */
    public function data(string $entity): QueryBuilder
    {
        return new QueryBuilder($this, $entity);
    }

    /**
     * Execute a batch transaction.
     *
     * @param array<int, array{kind: string, entity: string, id: string, data?: array}> $ops
     * @return array{txId: string}
     *
     * @throws Exception On server or network errors.
     */
    public function transact(array $ops): array
    {
        return $this->post('/api/transact', ['ops' => $ops]);
    }

    /**
     * Call a server-side function by name.
     *
     * @param string             $name The registered function name.
     * @param array<string, mixed> $args Arguments to pass to the function.
     * @return mixed The function's return value.
     *
     * @throws Exception On server or network errors.
     */
    public function fn(string $name, array $args = []): mixed
    {
        $result = $this->post("/api/fn/{$name}", $args);

        return $result['result'] ?? $result;
    }

    /**
     * Get the storage client for file uploads and management.
     */
    public function storage(): StorageClient
    {
        if ($this->storageClient === null) {
            $this->storageClient = new StorageClient($this);
        }

        return $this->storageClient;
    }

    /* ---------------------------------------------------------------------- */
    /*  Internal HTTP helpers                                                  */
    /* ---------------------------------------------------------------------- */

    /**
     * Set the current authentication token.
     *
     * @internal Used by AuthClient after successful authentication.
     */
    public function setToken(?string $token): void
    {
        $this->token = $token;
    }

    /**
     * Get the current authentication token.
     *
     * @internal
     */
    public function getToken(): ?string
    {
        return $this->token;
    }

    /**
     * Send a POST request to the DarshJDB server.
     *
     * @internal
     *
     * @param string              $path    API endpoint path.
     * @param array<string, mixed> $body   Request body (will be JSON-encoded).
     * @return array<string, mixed>
     *
     * @throws Exception
     */
    public function post(string $path, array $body = []): array
    {
        return $this->request('POST', $path, $body);
    }

    /**
     * Send a GET request to the DarshJDB server.
     *
     * @internal
     *
     * @param string                    $path  API endpoint path.
     * @param array<string, string|int> $query Query string parameters.
     * @return array<string, mixed>
     *
     * @throws Exception
     */
    public function get(string $path, array $query = []): array
    {
        return $this->request('GET', $path, query: $query);
    }

    /**
     * Send a DELETE request to the DarshJDB server.
     *
     * @internal
     *
     * @param string              $path API endpoint path.
     * @param array<string, mixed> $body Request body (will be JSON-encoded).
     * @return array<string, mixed>
     *
     * @throws Exception
     */
    public function delete(string $path, array $body = []): array
    {
        return $this->request('DELETE', $path, $body);
    }

    /**
     * Send a multipart POST request (used for file uploads).
     *
     * @internal
     *
     * @param string                                    $path      API endpoint path.
     * @param array<int, array{name: string, contents: mixed, filename?: string}> $multipart Multipart form data.
     * @return array<string, mixed>
     *
     * @throws Exception
     */
    public function postMultipart(string $path, array $multipart): array
    {
        try {
            $response = $this->http->request('POST', $path, [
                'headers'   => $this->buildHeaders(contentType: false),
                'multipart' => $multipart,
            ]);

            /** @var array<string, mixed> $decoded */
            $decoded = json_decode((string) $response->getBody(), true, 512, JSON_THROW_ON_ERROR);

            return $decoded;
        } catch (GuzzleException $e) {
            throw Exception::fromGuzzle($e);
        } catch (\JsonException $e) {
            throw new Exception('Invalid JSON response from server.', 0, $e);
        }
    }

    /**
     * Build the authorization and content-type headers.
     *
     * @param bool $contentType Whether to include the Content-Type header.
     * @return array<string, string>
     */
    private function buildHeaders(bool $contentType = true): array
    {
        $headers = [
            'X-Api-Key' => $this->apiKey,
            'Accept'     => 'application/json',
        ];

        if ($contentType) {
            $headers['Content-Type'] = 'application/json';
        }

        if ($this->token !== null) {
            $headers['Authorization'] = "Bearer {$this->token}";
        }

        return $headers;
    }

    /**
     * Execute an HTTP request against the DarshJDB server.
     *
     * @param string              $method HTTP method.
     * @param string              $path   API endpoint path.
     * @param array<string, mixed> $body   Request body for POST/PUT/DELETE.
     * @param array<string, string|int> $query  Query string parameters for GET.
     * @return array<string, mixed>
     *
     * @throws Exception
     */
    private function request(
        string $method,
        string $path,
        array $body = [],
        array $query = [],
    ): array {
        $options = [
            'headers' => $this->buildHeaders(),
        ];

        if ($method !== 'GET' && !empty($body)) {
            $options[RequestOptions::JSON] = $body;
        }

        if (!empty($query)) {
            $options[RequestOptions::QUERY] = $query;
        }

        try {
            $response = $this->http->request($method, $path, $options);

            /** @var array<string, mixed> $decoded */
            $decoded = json_decode((string) $response->getBody(), true, 512, JSON_THROW_ON_ERROR);

            return $decoded;
        } catch (GuzzleException $e) {
            throw Exception::fromGuzzle($e);
        } catch (\JsonException $e) {
            throw new Exception('Invalid JSON response from server.', 0, $e);
        }
    }
}
