<?php

declare(strict_types=1);

namespace Darshjdb\Tests;

use Darshjdb\Client;
use Darshjdb\Exception;
use GuzzleHttp\Client as HttpClient;
use GuzzleHttp\Exception\RequestException;
use GuzzleHttp\Handler\MockHandler;
use GuzzleHttp\HandlerStack;
use GuzzleHttp\Middleware;
use GuzzleHttp\Psr7\Request;
use GuzzleHttp\Psr7\Response;
use PHPUnit\Framework\TestCase;

/**
 * Verifies that the SDK targets routes that actually exist on the server
 * (packages/server/src/api/rest.rs) with the methods those routes accept.
 */
final class RoutingTest extends TestCase
{
    /** @var array<int, array{request: \Psr\Http\Message\RequestInterface}> */
    private array $history = [];

    /**
     * Build a client whose transport is a mock returning the given responses.
     *
     * @param array<int, Response> $responses
     */
    private function makeClient(array $responses): Client
    {
        $client = new Client([
            'serverUrl' => 'http://localhost:7700',
            'apiKey'    => 'test-key',
        ]);

        $this->history = [];
        $stack = HandlerStack::create(new MockHandler($responses));
        $stack->push(Middleware::history($this->history));

        $property = new \ReflectionProperty(Client::class, 'http');
        $property->setAccessible(true);
        $property->setValue($client, new HttpClient([
            'base_uri' => 'http://localhost:7700',
            'handler'  => $stack,
        ]));

        return $client;
    }

    private function jsonResponse(int $status, array $body): Response
    {
        return new Response($status, ['Content-Type' => 'application/json'], json_encode($body));
    }

    private function lastRequest(): \Psr\Http\Message\RequestInterface
    {
        return $this->history[count($this->history) - 1]['request'];
    }

    /* ---------------------------------------------------------------------- */
    /*  Data routes                                                            */
    /* ---------------------------------------------------------------------- */

    public function testQueryHitsQueryRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['data' => []])]);
        $client->query(['collection' => 'posts']);

        $this->assertSame('POST', $this->lastRequest()->getMethod());
        $this->assertSame('/api/query', $this->lastRequest()->getUri()->getPath());
    }

    public function testTransactHitsMutateRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['txId' => 'tx-1'])]);
        $client->transact([
            ['op' => 'insert', 'entity' => 'posts', 'data' => ['title' => 'Hi']],
        ]);

        $request = $this->lastRequest();

        $this->assertSame('POST', $request->getMethod());
        $this->assertSame('/api/mutate', $request->getUri()->getPath());
        $this->assertSame(
            ['mutations' => [['op' => 'insert', 'entity' => 'posts', 'data' => ['title' => 'Hi']]]],
            json_decode((string) $request->getBody(), true),
        );
    }

    public function testCreateHitsCollectionRouteWithPost(): void
    {
        $client = $this->makeClient([$this->jsonResponse(201, ['id' => 'p1'])]);
        $client->data('posts')->create(['title' => 'Hi']);

        $this->assertSame('POST', $this->lastRequest()->getMethod());
        $this->assertSame('/api/data/posts', $this->lastRequest()->getUri()->getPath());
    }

    public function testUpdateUsesPatchBecauseTheRouteIsPatchOnly(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['id' => 'p1'])]);
        $client->data('posts')->update('p1', ['title' => 'Updated']);

        $this->assertSame('PATCH', $this->lastRequest()->getMethod());
        $this->assertSame('/api/data/posts/p1', $this->lastRequest()->getUri()->getPath());
    }

    public function testDeleteHitsEntityRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, [])]);
        $client->data('posts')->delete('p1');

        $this->assertSame('DELETE', $this->lastRequest()->getMethod());
        $this->assertSame('/api/data/posts/p1', $this->lastRequest()->getUri()->getPath());
    }

    public function testFnHitsFunctionRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['result' => 42])]);

        $this->assertSame(42, $client->fn('sum', ['a' => 1]));
        $this->assertSame('/api/fn/sum', $this->lastRequest()->getUri()->getPath());
    }

    /* ---------------------------------------------------------------------- */
    /*  Auth routes                                                            */
    /* ---------------------------------------------------------------------- */

    public function testSignInStoresSnakeCaseAccessToken(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(200, [
                'access_token'  => 'jwt-abc',
                'refresh_token' => 'ref-abc',
            ]),
        ]);

        $client->auth()->signIn('a@b.com', 'secret');

        $this->assertSame('/api/auth/signin', $this->lastRequest()->getUri()->getPath());
        $this->assertSame('jwt-abc', $client->getToken());
    }

    public function testSignUpStoresSnakeCaseAccessToken(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(200, ['access_token' => 'jwt-signup']),
        ]);

        $client->auth()->signUp('a@b.com', 'secret');

        $this->assertSame('/api/auth/signup', $this->lastRequest()->getUri()->getPath());
        $this->assertSame('jwt-signup', $client->getToken());
    }

    public function testOAuthHitsPerProviderRoute(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(200, ['access_token' => 'jwt-oauth']),
        ]);

        $client->auth()->signInWithOAuth('google', 'code-1', 'state-1', 'verifier-1');

        $request = $this->lastRequest();

        $this->assertSame('POST', $request->getMethod());
        $this->assertSame('/api/auth/oauth/google', $request->getUri()->getPath());
        $this->assertSame(
            ['code' => 'code-1', 'state' => 'state-1', 'pkce_verifier' => 'verifier-1'],
            json_decode((string) $request->getBody(), true),
        );
        $this->assertSame('jwt-oauth', $client->getToken());
    }

    public function testRefreshSendsSnakeCaseRefreshToken(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(200, ['access_token' => 'jwt-new', 'refresh_token' => 'ref-new']),
        ]);

        $client->auth()->refresh('ref-old');

        $request = $this->lastRequest();

        $this->assertSame('/api/auth/refresh', $request->getUri()->getPath());
        $this->assertSame(
            ['refresh_token' => 'ref-old'],
            json_decode((string) $request->getBody(), true),
        );
        $this->assertSame('jwt-new', $client->getToken());
    }

    public function testSignOutClearsToken(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['ok' => true])]);
        $client->setToken('jwt-abc');

        $client->auth()->signOut();

        $this->assertSame('/api/auth/signout', $this->lastRequest()->getUri()->getPath());
        $this->assertNull($client->getToken());
    }

    public function testGetUserHitsMeRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['id' => 'u1'])]);

        $this->assertSame(['id' => 'u1'], $client->auth()->getUser());
        $this->assertSame('GET', $this->lastRequest()->getMethod());
        $this->assertSame('/api/auth/me', $this->lastRequest()->getUri()->getPath());
    }

    /* ---------------------------------------------------------------------- */
    /*  Storage routes                                                         */
    /* ---------------------------------------------------------------------- */

    public function testGetUrlAsksTheObjectRouteForASignedUrl(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(200, ['signed_url' => 'http://localhost:7700/api/storage/a.jpg?sig=x']),
        ]);

        $url = $client->storage()->getUrl('/avatars/a.jpg');

        $request = $this->lastRequest();

        $this->assertSame('GET', $request->getMethod());
        $this->assertSame('/api/storage/avatars/a.jpg', $request->getUri()->getPath());
        $this->assertSame('signed=true', $request->getUri()->getQuery());
        $this->assertSame('http://localhost:7700/api/storage/a.jpg?sig=x', $url);
    }

    public function testStorageDeleteHitsObjectRouteAndToleratesNoContent(): void
    {
        $client = $this->makeClient([new Response(204)]);

        $this->assertSame([], $client->storage()->delete('/avatars/a.jpg'));
        $this->assertSame('DELETE', $this->lastRequest()->getMethod());
        $this->assertSame('/api/storage/avatars/a.jpg', $this->lastRequest()->getUri()->getPath());
    }

    public function testStorageListHitsAdminStorageRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(200, ['files' => []])]);

        $client->storage()->list(50, 'cur-1');

        $request = $this->lastRequest();

        $this->assertSame('GET', $request->getMethod());
        $this->assertSame('/api/admin/storage', $request->getUri()->getPath());
        $this->assertSame('limit=50&cursor=cur-1', $request->getUri()->getQuery());
    }

    public function testStorageUploadHitsUploadRoute(): void
    {
        $client = $this->makeClient([$this->jsonResponse(201, ['path' => 'a.jpg'])]);

        $client->storage()->uploadRaw('/avatars/a.jpg', 'bytes', 'a.jpg');

        $this->assertSame('POST', $this->lastRequest()->getMethod());
        $this->assertSame('/api/storage/upload', $this->lastRequest()->getUri()->getPath());
    }

    /* ---------------------------------------------------------------------- */
    /*  Error envelope unwrapping                                              */
    /* ---------------------------------------------------------------------- */

    public function testNestedServerErrorMessageIsExtracted(): void
    {
        $client = $this->makeClient([
            $this->jsonResponse(404, [
                'error' => [
                    'code'    => 'NOT_FOUND',
                    'message' => 'Entity posts/p1 was not found.',
                    'status'  => 404,
                ],
            ]),
        ]);

        try {
            $client->data('posts')->delete('p1');
            $this->fail('Expected a Darshjdb\Exception.');
        } catch (Exception $e) {
            $this->assertSame('Entity posts/p1 was not found.', $e->getMessage());
            $this->assertSame(404, $e->getStatusCode());
        }
    }

    public function testFlatMessageErrorBodyStillWorks(): void
    {
        $e = Exception::fromGuzzle(new RequestException(
            'Client error',
            new Request('GET', '/api/auth/me'),
            new Response(401, [], json_encode(['message' => 'Token expired.'])),
        ));

        $this->assertSame('Token expired.', $e->getMessage());
    }

    public function testStringErrorBodyStillWorks(): void
    {
        $e = Exception::fromGuzzle(new RequestException(
            'Client error',
            new Request('GET', '/api/auth/me'),
            new Response(403, [], json_encode(['error' => 'forbidden'])),
        ));

        $this->assertSame('forbidden', $e->getMessage());
    }

    public function testNonJsonErrorBodyFallsBackToGuzzleMessage(): void
    {
        $e = Exception::fromGuzzle(new RequestException(
            'Server error: 502 Bad Gateway',
            new Request('GET', '/api/auth/me'),
            new Response(502, [], '<html>bad gateway</html>'),
        ));

        $this->assertSame('Server error: 502 Bad Gateway', $e->getMessage());
        $this->assertSame(502, $e->getStatusCode());
    }
}
