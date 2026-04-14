<?php

declare(strict_types=1);

namespace Darshjdb\Tests;

use Darshjdb\Client;
use Darshjdb\QueryBuilder;
use PHPUnit\Framework\TestCase;

/**
 * Tests for the QueryBuilder fluent interface and descriptor output.
 *
 * Since buildDescriptor() is private, we use reflection to invoke it.
 * This keeps the public API clean while still verifying internal structure.
 */
final class QueryBuilderTest extends TestCase
{
    private function makeClient(): Client
    {
        return new Client([
            'serverUrl' => 'http://localhost:7700',
            'apiKey'    => 'test-key',
        ]);
    }

    private function makeBuilder(string $entity = 'posts'): QueryBuilder
    {
        return new QueryBuilder($this->makeClient(), $entity);
    }

    /**
     * Use reflection to call the private buildDescriptor() method.
     *
     * @return array<string, mixed>
     */
    private function buildDescriptor(QueryBuilder $qb): array
    {
        $ref = new \ReflectionMethod($qb, 'buildDescriptor');
        $ref->setAccessible(true);

        return $ref->invoke($qb);
    }

    /* ---------------------------------------------------------------------- */
    /*  Constructor / base descriptor                                         */
    /* ---------------------------------------------------------------------- */

    public function testEmptyBuilderReturnsCollectionOnly(): void
    {
        $qb = $this->makeBuilder('users');
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(['collection' => 'users'], $desc);
    }

    public function testDifferentEntityNames(): void
    {
        foreach (['posts', 'comments', 'orders', 'line-items'] as $entity) {
            $desc = $this->buildDescriptor($this->makeBuilder($entity));
            $this->assertSame($entity, $desc['collection']);
        }
    }

    /* ---------------------------------------------------------------------- */
    /*  where()                                                               */
    /* ---------------------------------------------------------------------- */

    public function testSingleWhere(): void
    {
        $qb = $this->makeBuilder()->where('published', '=', true);
        $desc = $this->buildDescriptor($qb);

        $this->assertCount(1, $desc['where']);
        $this->assertSame([
            'field' => 'published',
            'op'    => '=',
            'value' => true,
        ], $desc['where'][0]);
    }

    public function testMultipleWheres(): void
    {
        $qb = $this->makeBuilder()
            ->where('published', '=', true)
            ->where('category', 'in', ['tech', 'design'])
            ->where('views', '>', 100);

        $desc = $this->buildDescriptor($qb);

        $this->assertCount(3, $desc['where']);

        $this->assertSame('published', $desc['where'][0]['field']);
        $this->assertSame('=', $desc['where'][0]['op']);
        $this->assertTrue($desc['where'][0]['value']);

        $this->assertSame('category', $desc['where'][1]['field']);
        $this->assertSame('in', $desc['where'][1]['op']);
        $this->assertSame(['tech', 'design'], $desc['where'][1]['value']);

        $this->assertSame('views', $desc['where'][2]['field']);
        $this->assertSame('>', $desc['where'][2]['op']);
        $this->assertSame(100, $desc['where'][2]['value']);
    }

    public function testWhereWithAllOperators(): void
    {
        $operators = ['=', '!=', '>', '>=', '<', '<=', 'in', 'not-in', 'contains', 'starts-with'];

        foreach ($operators as $op) {
            $qb = $this->makeBuilder()->where('field', $op, 'val');
            $desc = $this->buildDescriptor($qb);

            $this->assertSame($op, $desc['where'][0]['op'], "Operator '{$op}' not stored correctly");
        }
    }

    public function testWhereWithNullValue(): void
    {
        $qb = $this->makeBuilder()->where('deletedAt', '=', null);
        $desc = $this->buildDescriptor($qb);

        $this->assertNull($desc['where'][0]['value']);
    }

    public function testWhereWithNumericValues(): void
    {
        $qb = $this->makeBuilder()
            ->where('price', '>=', 9.99)
            ->where('quantity', '=', 0);

        $desc = $this->buildDescriptor($qb);

        $this->assertSame(9.99, $desc['where'][0]['value']);
        $this->assertSame(0, $desc['where'][1]['value']);
    }

    /* ---------------------------------------------------------------------- */
    /*  orderBy()                                                             */
    /* ---------------------------------------------------------------------- */

    public function testOrderByDefaultAsc(): void
    {
        $qb = $this->makeBuilder()->orderBy('name');
        $desc = $this->buildDescriptor($qb);

        $this->assertCount(1, $desc['order']);
        $this->assertSame([
            'field'     => 'name',
            'direction' => 'asc',
        ], $desc['order'][0]);
    }

    public function testOrderByDesc(): void
    {
        $qb = $this->makeBuilder()->orderBy('createdAt', 'desc');
        $desc = $this->buildDescriptor($qb);

        $this->assertSame('desc', $desc['order'][0]['direction']);
    }

    public function testMultipleOrderBy(): void
    {
        $qb = $this->makeBuilder()
            ->orderBy('priority', 'desc')
            ->orderBy('name', 'asc');

        $desc = $this->buildDescriptor($qb);

        $this->assertCount(2, $desc['order']);
        $this->assertSame('priority', $desc['order'][0]['field']);
        $this->assertSame('desc', $desc['order'][0]['direction']);
        $this->assertSame('name', $desc['order'][1]['field']);
        $this->assertSame('asc', $desc['order'][1]['direction']);
    }

    /* ---------------------------------------------------------------------- */
    /*  limit()                                                               */
    /* ---------------------------------------------------------------------- */

    public function testLimit(): void
    {
        $qb = $this->makeBuilder()->limit(25);
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(25, $desc['limit']);
    }

    public function testLimitZero(): void
    {
        $qb = $this->makeBuilder()->limit(0);
        $desc = $this->buildDescriptor($qb);

        // 0 is explicitly set, should be in descriptor
        $this->assertSame(0, $desc['limit']);
    }

    public function testNoLimitOmitsKey(): void
    {
        $desc = $this->buildDescriptor($this->makeBuilder());
        $this->assertArrayNotHasKey('limit', $desc);
    }

    /* ---------------------------------------------------------------------- */
    /*  offset()                                                              */
    /* ---------------------------------------------------------------------- */

    public function testOffset(): void
    {
        $qb = $this->makeBuilder()->offset(40);
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(40, $desc['offset']);
    }

    public function testOffsetZero(): void
    {
        $qb = $this->makeBuilder()->offset(0);
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(0, $desc['offset']);
    }

    public function testNoOffsetOmitsKey(): void
    {
        $desc = $this->buildDescriptor($this->makeBuilder());
        $this->assertArrayNotHasKey('offset', $desc);
    }

    /* ---------------------------------------------------------------------- */
    /*  select()                                                              */
    /* ---------------------------------------------------------------------- */

    public function testSelect(): void
    {
        $qb = $this->makeBuilder()->select(['id', 'title', 'summary']);
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(['id', 'title', 'summary'], $desc['select']);
    }

    public function testSelectEmptyArray(): void
    {
        // An empty select should still be set (explicit projection of nothing)
        $qb = $this->makeBuilder()->select([]);
        $desc = $this->buildDescriptor($qb);

        // Empty array is falsy, so buildDescriptor will use selectFields === null check
        // Since selectFields is set to [], it is not null, so it should be included
        $this->assertArrayHasKey('select', $desc);
        $this->assertSame([], $desc['select']);
    }

    public function testNoSelectOmitsKey(): void
    {
        $desc = $this->buildDescriptor($this->makeBuilder());
        $this->assertArrayNotHasKey('select', $desc);
    }

    /* ---------------------------------------------------------------------- */
    /*  Fluent chaining                                                       */
    /* ---------------------------------------------------------------------- */

    public function testFluentChainReturnsSameInstance(): void
    {
        $qb = $this->makeBuilder();

        $this->assertSame($qb, $qb->where('a', '=', 1));
        $this->assertSame($qb, $qb->orderBy('b'));
        $this->assertSame($qb, $qb->limit(10));
        $this->assertSame($qb, $qb->offset(5));
        $this->assertSame($qb, $qb->select(['a', 'b']));
    }

    public function testFullChainProducesCorrectDescriptor(): void
    {
        $qb = $this->makeBuilder('posts')
            ->where('published', '=', true)
            ->where('category', 'in', ['tech', 'design'])
            ->orderBy('createdAt', 'desc')
            ->limit(20)
            ->offset(40)
            ->select(['id', 'title', 'summary']);

        $desc = $this->buildDescriptor($qb);

        $this->assertSame('posts', $desc['collection']);

        $this->assertCount(2, $desc['where']);
        $this->assertSame('published', $desc['where'][0]['field']);
        $this->assertSame('category', $desc['where'][1]['field']);

        $this->assertCount(1, $desc['order']);
        $this->assertSame('createdAt', $desc['order'][0]['field']);
        $this->assertSame('desc', $desc['order'][0]['direction']);

        $this->assertSame(20, $desc['limit']);
        $this->assertSame(40, $desc['offset']);
        $this->assertSame(['id', 'title', 'summary'], $desc['select']);
    }

    /* ---------------------------------------------------------------------- */
    /*  Descriptor key omission when unused                                   */
    /* ---------------------------------------------------------------------- */

    public function testUnusedFieldsAreOmittedFromDescriptor(): void
    {
        $qb = $this->makeBuilder('items')->where('active', '=', true);
        $desc = $this->buildDescriptor($qb);

        $this->assertArrayHasKey('collection', $desc);
        $this->assertArrayHasKey('where', $desc);
        $this->assertArrayNotHasKey('order', $desc);
        $this->assertArrayNotHasKey('limit', $desc);
        $this->assertArrayNotHasKey('offset', $desc);
        $this->assertArrayNotHasKey('select', $desc);
    }

    public function testOnlyOrderByPresent(): void
    {
        $qb = $this->makeBuilder('items')->orderBy('name');
        $desc = $this->buildDescriptor($qb);

        $this->assertArrayHasKey('order', $desc);
        $this->assertArrayNotHasKey('where', $desc);
        $this->assertArrayNotHasKey('limit', $desc);
    }

    public function testOnlyLimitAndOffset(): void
    {
        $qb = $this->makeBuilder('items')->limit(10)->offset(20);
        $desc = $this->buildDescriptor($qb);

        $this->assertSame(10, $desc['limit']);
        $this->assertSame(20, $desc['offset']);
        $this->assertArrayNotHasKey('where', $desc);
        $this->assertArrayNotHasKey('order', $desc);
        $this->assertArrayNotHasKey('select', $desc);
    }
}
