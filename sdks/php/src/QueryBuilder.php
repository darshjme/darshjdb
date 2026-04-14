<?php

declare(strict_types=1);

namespace Darshjdb;

/**
 * Fluent query builder for DarshJDB collections.
 *
 * Provides a chainable API for constructing queries, and CRUD shortcuts
 * for individual entities.
 *
 * Usage:
 *   // Read
 *   $posts = $db->data('posts')
 *       ->where('published', '=', true)
 *       ->where('category', 'in', ['tech', 'design'])
 *       ->orderBy('createdAt', 'desc')
 *       ->limit(20)
 *       ->offset(40)
 *       ->select(['id', 'title', 'summary'])
 *       ->get();
 *
 *   // Create
 *   $post = $db->data('posts')->create(['title' => 'Hello', 'body' => '...']);
 *
 *   // Update
 *   $db->data('posts')->update('post-id', ['title' => 'Updated Title']);
 *
 *   // Delete
 *   $db->data('posts')->delete('post-id');
 */
class QueryBuilder
{
    /** @var array<int, array{field: string, op: string, value: mixed}> */
    private array $wheres = [];

    /** @var array<int, array{field: string, direction: string}> */
    private array $orders = [];

    private ?int $limitValue = null;
    private ?int $offsetValue = null;

    /** @var string[]|null */
    private ?array $selectFields = null;

    public function __construct(
        private readonly Client $client,
        private readonly string $entity,
    ) {
    }

    /**
     * Add a where clause to the query.
     *
     * @param string $field The field name to filter on.
     * @param string $op    Comparison operator: =, !=, >, >=, <, <=, in, not-in, contains, starts-with.
     * @param mixed  $value The value to compare against.
     * @return $this
     */
    public function where(string $field, string $op, mixed $value): static
    {
        $this->wheres[] = [
            'field' => $field,
            'op'    => $op,
            'value' => $value,
        ];

        return $this;
    }

    /**
     * Add an order-by clause.
     *
     * @param string $field     The field to sort by.
     * @param string $direction Sort direction: 'asc' or 'desc' (default: 'asc').
     * @return $this
     */
    public function orderBy(string $field, string $direction = 'asc'): static
    {
        $this->orders[] = [
            'field'     => $field,
            'direction' => $direction,
        ];

        return $this;
    }

    /**
     * Limit the number of results returned.
     *
     * @param int $limit Maximum number of records.
     * @return $this
     */
    public function limit(int $limit): static
    {
        $this->limitValue = $limit;

        return $this;
    }

    /**
     * Skip a number of results (for pagination).
     *
     * @param int $offset Number of records to skip.
     * @return $this
     */
    public function offset(int $offset): static
    {
        $this->offsetValue = $offset;

        return $this;
    }

    /**
     * Select specific fields to return (projection).
     *
     * @param string[] $fields List of field names to include.
     * @return $this
     */
    public function select(array $fields): static
    {
        $this->selectFields = $fields;

        return $this;
    }

    /**
     * Execute the query and return matching records.
     *
     * @return array{data: array<int, array<string, mixed>>, txId: string}
     *
     * @throws Exception On server or network errors.
     */
    public function get(): array
    {
        return $this->client->query($this->buildDescriptor());
    }

    /**
     * Create a new record in this collection.
     *
     * @param array<string, mixed> $data The record data.
     * @return array<string, mixed> The created record (with server-assigned id).
     *
     * @throws Exception On validation or server errors.
     */
    public function create(array $data): array
    {
        return $this->client->post("/api/data/{$this->entity}", $data);
    }

    /**
     * Update an existing record by ID.
     *
     * @param string               $id   The record's unique identifier.
     * @param array<string, mixed> $data Fields to update (merge semantics).
     * @return array<string, mixed> The updated record.
     *
     * @throws Exception On not-found or server errors.
     */
    public function update(string $id, array $data): array
    {
        return $this->client->post("/api/data/{$this->entity}/{$id}", $data);
    }

    /**
     * Delete a record by ID.
     *
     * @param string $id The record's unique identifier.
     * @return array<string, mixed> Server acknowledgement.
     *
     * @throws Exception On not-found or server errors.
     */
    public function delete(string $id): array
    {
        return $this->client->delete("/api/data/{$this->entity}/{$id}");
    }

    /**
     * Build the query descriptor array for the server.
     *
     * @return array<string, mixed>
     */
    private function buildDescriptor(): array
    {
        $descriptor = [
            'collection' => $this->entity,
        ];

        if (!empty($this->wheres)) {
            $descriptor['where'] = $this->wheres;
        }

        if (!empty($this->orders)) {
            $descriptor['order'] = $this->orders;
        }

        if ($this->limitValue !== null) {
            $descriptor['limit'] = $this->limitValue;
        }

        if ($this->offsetValue !== null) {
            $descriptor['offset'] = $this->offsetValue;
        }

        if ($this->selectFields !== null) {
            $descriptor['select'] = $this->selectFields;
        }

        return $descriptor;
    }
}
