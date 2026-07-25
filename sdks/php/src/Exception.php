<?php

declare(strict_types=1);

namespace Darshjdb;

use GuzzleHttp\Exception\GuzzleException;
use GuzzleHttp\Exception\RequestException;

/**
 * Base exception for all DarshJDB SDK errors.
 *
 * Wraps Guzzle transport errors and provides structured access to
 * server-returned error details.
 */
class Exception extends \RuntimeException
{
    private ?int $statusCode;

    /** @var array<string, mixed> */
    private array $errorBody;

    /**
     * @param string                $message    Human-readable error message.
     * @param int                   $code       Error code (usually HTTP status).
     * @param \Throwable|null       $previous   Previous exception for chaining.
     * @param int|null              $statusCode HTTP status code from the server.
     * @param array<string, mixed>  $errorBody  Parsed error body from the server.
     */
    public function __construct(
        string $message = '',
        int $code = 0,
        ?\Throwable $previous = null,
        ?int $statusCode = null,
        array $errorBody = [],
    ) {
        parent::__construct($message, $code, $previous);
        $this->statusCode = $statusCode;
        $this->errorBody = $errorBody;
    }

    /**
     * Create a Exception from a Guzzle exception.
     */
    public static function fromGuzzle(GuzzleException $e): self
    {
        $statusCode = null;
        $errorBody = [];

        if ($e instanceof RequestException && $e->hasResponse()) {
            $response = $e->getResponse();
            $statusCode = $response?->getStatusCode();

            try {
                /** @var array<string, mixed> $errorBody */
                $errorBody = json_decode(
                    (string) $response?->getBody(),
                    true,
                    512,
                    JSON_THROW_ON_ERROR,
                );
            } catch (\JsonException) {
                // Non-JSON error body — keep empty.
            }
        }

        return new self(
            message: self::extractMessage($errorBody) ?? $e->getMessage(),
            code: (int) $e->getCode(),
            previous: $e,
            statusCode: $statusCode,
            errorBody: $errorBody,
        );
    }

    /**
     * Pull the human-readable message out of a server error body.
     *
     * The server wraps errors in `{"error": {"code": ..., "message": ...}}`,
     * so the nested object must be unwrapped before the message is read.
     * Flat `{"message": ...}` and `{"error": "..."}` bodies are also accepted.
     *
     * @param array<string, mixed> $errorBody Parsed error body from the server.
     */
    private static function extractMessage(array $errorBody): ?string
    {
        $error = $errorBody['error'] ?? null;

        if (is_array($error) && isset($error['message']) && is_string($error['message'])) {
            return $error['message'];
        }

        if (isset($errorBody['message']) && is_string($errorBody['message'])) {
            return $errorBody['message'];
        }

        if (is_string($error) && $error !== '') {
            return $error;
        }

        return null;
    }

    /**
     * Get the HTTP status code (null if the error was not an HTTP response).
     */
    public function getStatusCode(): ?int
    {
        return $this->statusCode;
    }

    /**
     * Get the parsed error body from the server.
     *
     * @return array<string, mixed>
     */
    public function getErrorBody(): array
    {
        return $this->errorBody;
    }
}
