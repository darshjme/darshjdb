<?php

declare(strict_types=1);

namespace Darshan\Tests;

use Darshan\DarshanException;
use PHPUnit\Framework\TestCase;

final class ExceptionTest extends TestCase
{
    /* ---------------------------------------------------------------------- */
    /*  Hierarchy                                                             */
    /* ---------------------------------------------------------------------- */

    public function testExtendsRuntimeException(): void
    {
        $ex = new DarshanException('test');

        $this->assertInstanceOf(\RuntimeException::class, $ex);
        $this->assertInstanceOf(\Exception::class, $ex);
        $this->assertInstanceOf(\Throwable::class, $ex);
    }

    /* ---------------------------------------------------------------------- */
    /*  Constructor defaults                                                  */
    /* ---------------------------------------------------------------------- */

    public function testDefaultValues(): void
    {
        $ex = new DarshanException();

        $this->assertSame('', $ex->getMessage());
        $this->assertSame(0, $ex->getCode());
        $this->assertNull($ex->getPrevious());
        $this->assertNull($ex->getStatusCode());
        $this->assertSame([], $ex->getErrorBody());
    }

    public function testMessageOnly(): void
    {
        $ex = new DarshanException('Something went wrong');

        $this->assertSame('Something went wrong', $ex->getMessage());
        $this->assertNull($ex->getStatusCode());
        $this->assertSame([], $ex->getErrorBody());
    }

    /* ---------------------------------------------------------------------- */
    /*  getStatusCode()                                                       */
    /* ---------------------------------------------------------------------- */

    public function testGetStatusCode(): void
    {
        $ex = new DarshanException('Not found', 404, null, 404);

        $this->assertSame(404, $ex->getStatusCode());
    }

    public function testStatusCodeNullWhenNotProvided(): void
    {
        $ex = new DarshanException('Network error');

        $this->assertNull($ex->getStatusCode());
    }

    public function testStatusCodeVariousHttpCodes(): void
    {
        $codes = [400, 401, 403, 404, 409, 422, 429, 500, 502, 503];

        foreach ($codes as $code) {
            $ex = new DarshanException("Error {$code}", $code, null, $code);
            $this->assertSame($code, $ex->getStatusCode(), "Status code {$code} mismatch");
        }
    }

    /* ---------------------------------------------------------------------- */
    /*  getErrorBody()                                                        */
    /* ---------------------------------------------------------------------- */

    public function testGetErrorBody(): void
    {
        $body = [
            'error'   => 'validation_failed',
            'message' => 'Email is required',
            'details' => ['field' => 'email'],
        ];

        $ex = new DarshanException('Validation failed', 422, null, 422, $body);

        $this->assertSame($body, $ex->getErrorBody());
    }

    public function testEmptyErrorBodyByDefault(): void
    {
        $ex = new DarshanException('Error');

        $this->assertSame([], $ex->getErrorBody());
        $this->assertIsArray($ex->getErrorBody());
    }

    /* ---------------------------------------------------------------------- */
    /*  Exception chaining                                                    */
    /* ---------------------------------------------------------------------- */

    public function testPreviousException(): void
    {
        $prev = new \RuntimeException('Original error');
        $ex = new DarshanException('Wrapped', 0, $prev);

        $this->assertSame($prev, $ex->getPrevious());
    }

    /* ---------------------------------------------------------------------- */
    /*  Full constructor                                                      */
    /* ---------------------------------------------------------------------- */

    public function testFullConstructor(): void
    {
        $prev = new \Exception('inner');
        $body = ['error' => 'rate_limited', 'retryAfter' => 30];

        $ex = new DarshanException(
            message: 'Rate limited',
            code: 429,
            previous: $prev,
            statusCode: 429,
            errorBody: $body,
        );

        $this->assertSame('Rate limited', $ex->getMessage());
        $this->assertSame(429, $ex->getCode());
        $this->assertSame($prev, $ex->getPrevious());
        $this->assertSame(429, $ex->getStatusCode());
        $this->assertSame($body, $ex->getErrorBody());
    }

    /* ---------------------------------------------------------------------- */
    /*  Catchable as parent types                                             */
    /* ---------------------------------------------------------------------- */

    public function testCatchableAsRuntimeException(): void
    {
        $caught = false;

        try {
            throw new DarshanException('test', 500, null, 500);
        } catch (\RuntimeException $e) {
            $caught = true;
            $this->assertInstanceOf(DarshanException::class, $e);
        }

        $this->assertTrue($caught, 'DarshanException should be catchable as RuntimeException');
    }

    public function testCatchableAsException(): void
    {
        $caught = false;

        try {
            throw new DarshanException('test');
        } catch (\Exception $e) {
            $caught = true;
        }

        $this->assertTrue($caught, 'DarshanException should be catchable as Exception');
    }

    /* ---------------------------------------------------------------------- */
    /*  Code vs StatusCode independence                                       */
    /* ---------------------------------------------------------------------- */

    public function testCodeAndStatusCodeCanDiffer(): void
    {
        // code comes from Guzzle's exception code, statusCode from HTTP response
        $ex = new DarshanException('Error', 0, null, 503);

        $this->assertSame(0, $ex->getCode());
        $this->assertSame(503, $ex->getStatusCode());
    }
}
