<?php

declare(strict_types=1);

namespace Darshjdb;

/**
 * File storage client for DarshJDB.
 *
 * Provides upload, URL retrieval, and deletion for files stored on the server.
 *
 * Usage:
 *   $storage = $db->storage();
 *
 *   // Upload a file
 *   $result = $storage->upload('/avatars/photo.jpg', '/path/to/photo.jpg');
 *   echo $result['url'];
 *
 *   // Get a signed URL
 *   $url = $storage->getUrl('/avatars/photo.jpg');
 *
 *   // Delete a file
 *   $storage->delete('/avatars/photo.jpg');
 */
class StorageClient
{
    public function __construct(private readonly Client $client)
    {
    }

    /**
     * Upload a file to DarshJDB storage.
     *
     * @param string               $path        Remote storage path (e.g. '/avatars/photo.jpg').
     * @param string               $filePath    Local filesystem path to the file.
     * @param array<string, mixed> $options     Optional settings:
     *   - contentType (string): Override auto-detected MIME type.
     *   - metadata (array<string, string>): Custom metadata key-value pairs.
     * @return array{path: string, url: string, size: int, contentType: string}
     *
     * @throws Exception On upload failure.
     * @throws \InvalidArgumentException If the local file does not exist.
     */
    public function upload(string $path, string $filePath, array $options = []): array
    {
        if (!file_exists($filePath)) {
            throw new \InvalidArgumentException("File not found: {$filePath}");
        }

        $multipart = [
            [
                'name'     => 'file',
                'contents' => fopen($filePath, 'r'),
                'filename' => basename($filePath),
            ],
            [
                'name'     => 'path',
                'contents' => $path,
            ],
        ];

        if (isset($options['contentType'])) {
            $multipart[] = [
                'name'     => 'contentType',
                'contents' => $options['contentType'],
            ];
        }

        if (isset($options['metadata']) && is_array($options['metadata'])) {
            $multipart[] = [
                'name'     => 'metadata',
                'contents' => json_encode($options['metadata'], JSON_THROW_ON_ERROR),
            ];
        }

        return $this->client->postMultipart('/api/storage/upload', $multipart);
    }

    /**
     * Upload a file from a string or stream.
     *
     * @param string               $path        Remote storage path.
     * @param string|resource      $contents    File contents as string or stream resource.
     * @param string               $filename    The original filename.
     * @param array<string, mixed> $options     Optional settings (contentType, metadata).
     * @return array{path: string, url: string, size: int, contentType: string}
     *
     * @throws Exception On upload failure.
     */
    public function uploadRaw(string $path, mixed $contents, string $filename, array $options = []): array
    {
        $multipart = [
            [
                'name'     => 'file',
                'contents' => $contents,
                'filename' => $filename,
            ],
            [
                'name'     => 'path',
                'contents' => $path,
            ],
        ];

        if (isset($options['contentType'])) {
            $multipart[] = [
                'name'     => 'contentType',
                'contents' => $options['contentType'],
            ];
        }

        if (isset($options['metadata']) && is_array($options['metadata'])) {
            $multipart[] = [
                'name'     => 'metadata',
                'contents' => json_encode($options['metadata'], JSON_THROW_ON_ERROR),
            ];
        }

        return $this->client->postMultipart('/api/storage/upload', $multipart);
    }

    /**
     * Get the URL for a stored file.
     *
     * @param string $path   Remote storage path.
     * @param int    $expiry URL expiry time in seconds (default: 3600).
     * @return string The signed or public URL.
     *
     * @throws Exception On server errors.
     */
    public function getUrl(string $path, int $expiry = 3600): string
    {
        $result = $this->client->get('/api/storage/url', [
            'path'   => $path,
            'expiry' => $expiry,
        ]);

        return $result['url'] ?? '';
    }

    /**
     * Delete a file from storage.
     *
     * @param string $path Remote storage path.
     * @return array<string, mixed> Server acknowledgement.
     *
     * @throws Exception On server errors.
     */
    public function delete(string $path): array
    {
        return $this->client->delete('/api/storage/delete', [
            'path' => $path,
        ]);
    }

    /**
     * List files under a given prefix.
     *
     * @param string $prefix Directory prefix to list (e.g. '/avatars/').
     * @param int    $limit  Maximum files to return (default: 100).
     * @param string $cursor Pagination cursor from a previous response.
     * @return array{files: array<int, array{path: string, size: int, contentType: string, updatedAt: string}>, cursor?: string}
     *
     * @throws Exception On server errors.
     */
    public function list(string $prefix = '/', int $limit = 100, string $cursor = ''): array
    {
        $query = [
            'prefix' => $prefix,
            'limit'  => $limit,
        ];

        if ($cursor !== '') {
            $query['cursor'] = $cursor;
        }

        return $this->client->get('/api/storage/list', $query);
    }
}
