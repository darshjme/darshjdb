<?php

declare(strict_types=1);

namespace Darshan\Tests;

use Darshan\Client;
use Darshan\DarshanException;
use PHPUnit\Framework\TestCase;

final class ClientTest extends TestCase
{
    public function testClientInitialization(): void
    {
        $client = new Client([
            'serverUrl' => 'http://localhost:7700',
            'apiKey' => 'test-key',
        ]);

        $this->assertInstanceOf(Client::class, $client);
    }

    public function testClientHasAuth(): void
    {
        $client = new Client([
            'serverUrl' => 'http://localhost:7700',
            'apiKey' => 'test-key',
        ]);

        $this->assertNotNull($client->auth());
    }

    public function testClientHasStorage(): void
    {
        $client = new Client([
            'serverUrl' => 'http://localhost:7700',
            'apiKey' => 'test-key',
        ]);

        $this->assertNotNull($client->storage());
    }

    public function testExceptionHierarchy(): void
    {
        $this->assertTrue(is_subclass_of(DarshanException::class, \RuntimeException::class));
    }
}
