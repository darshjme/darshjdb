<?php

declare(strict_types=1);

namespace Darshjdb\Tests;

use Darshjdb\AuthClient;
use Darshjdb\Client;
use Darshjdb\Exception;
use Darshjdb\QueryBuilder;
use Darshjdb\StorageClient;
use PHPUnit\Framework\TestCase;

final class ClientTest extends TestCase
{
    private function makeClient(array $override = []): Client
    {
        return new Client(array_merge([
            'serverUrl' => 'http://localhost:7700',
            'apiKey'    => 'test-key',
        ], $override));
    }

    /* ---------------------------------------------------------------------- */
    /*  Initialization                                                        */
    /* ---------------------------------------------------------------------- */

    public function testClientInitialization(): void
    {
        $client = $this->makeClient();
        $this->assertInstanceOf(Client::class, $client);
    }

    public function testClientInitializationWithCustomTimeout(): void
    {
        $client = $this->makeClient(['timeout' => 60]);
        $this->assertInstanceOf(Client::class, $client);
    }

    public function testClientRequiresServerUrl(): void
    {
        $this->expectException(\InvalidArgumentException::class);
        $this->expectExceptionMessage('serverUrl is required');

        new Client(['apiKey' => 'test-key']);
    }

    public function testClientRequiresApiKey(): void
    {
        $this->expectException(\InvalidArgumentException::class);
        $this->expectExceptionMessage('apiKey is required');

        new Client(['serverUrl' => 'http://localhost:7700']);
    }

    public function testClientRejectsEmptyServerUrl(): void
    {
        $this->expectException(\InvalidArgumentException::class);

        new Client(['serverUrl' => '', 'apiKey' => 'test-key']);
    }

    public function testClientRejectsEmptyApiKey(): void
    {
        $this->expectException(\InvalidArgumentException::class);

        new Client(['serverUrl' => 'http://localhost:7700', 'apiKey' => '']);
    }

    public function testClientRejectsMissingBothFields(): void
    {
        $this->expectException(\InvalidArgumentException::class);

        new Client([]);
    }

    /* ---------------------------------------------------------------------- */
    /*  Sub-client accessors                                                  */
    /* ---------------------------------------------------------------------- */

    public function testAuthReturnsAuthClient(): void
    {
        $client = $this->makeClient();
        $auth = $client->auth();

        $this->assertInstanceOf(AuthClient::class, $auth);
    }

    public function testAuthReturnsSameInstance(): void
    {
        $client = $this->makeClient();

        $this->assertSame($client->auth(), $client->auth());
    }

    public function testStorageReturnsStorageClient(): void
    {
        $client = $this->makeClient();
        $storage = $client->storage();

        $this->assertInstanceOf(StorageClient::class, $storage);
    }

    public function testStorageReturnsSameInstance(): void
    {
        $client = $this->makeClient();

        $this->assertSame($client->storage(), $client->storage());
    }

    public function testDataReturnsQueryBuilder(): void
    {
        $client = $this->makeClient();
        $qb = $client->data('posts');

        $this->assertInstanceOf(QueryBuilder::class, $qb);
    }

    public function testDataReturnsFreshInstanceEachCall(): void
    {
        $client = $this->makeClient();

        $this->assertNotSame(
            $client->data('posts'),
            $client->data('posts'),
        );
    }

    /* ---------------------------------------------------------------------- */
    /*  Token management                                                      */
    /* ---------------------------------------------------------------------- */

    public function testTokenIsNullByDefault(): void
    {
        $client = $this->makeClient();

        $this->assertNull($client->getToken());
    }

    public function testSetAndGetToken(): void
    {
        $client = $this->makeClient();
        $client->setToken('abc123');

        $this->assertSame('abc123', $client->getToken());
    }

    public function testClearToken(): void
    {
        $client = $this->makeClient();
        $client->setToken('abc123');
        $client->setToken(null);

        $this->assertNull($client->getToken());
    }

    /* ---------------------------------------------------------------------- */
    /*  Query builder fluent interface via data()                              */
    /* ---------------------------------------------------------------------- */

    public function testDataQueryBuilderFluentChain(): void
    {
        $client = $this->makeClient();
        $qb = $client->data('users')
            ->where('age', '>=', 18)
            ->orderBy('name')
            ->limit(10)
            ->offset(20);

        $this->assertInstanceOf(QueryBuilder::class, $qb);
    }

    /* ---------------------------------------------------------------------- */
    /*  Exception hierarchy                                                   */
    /* ---------------------------------------------------------------------- */

    public function testExceptionExtendsRuntimeException(): void
    {
        $this->assertTrue(is_subclass_of(Exception::class, \RuntimeException::class));
    }

    public function testExceptionWithStatusCodeAndBody(): void
    {
        $ex = new Exception(
            message: 'Not found',
            code: 404,
            statusCode: 404,
            errorBody: ['error' => 'Resource not found'],
        );

        $this->assertSame(404, $ex->getStatusCode());
        $this->assertSame(['error' => 'Resource not found'], $ex->getErrorBody());
        $this->assertSame('Not found', $ex->getMessage());
        $this->assertSame(404, $ex->getCode());
    }
}
