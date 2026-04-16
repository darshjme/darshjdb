<?php

declare(strict_types=1);

namespace Darshjdb\Laravel;

use Illuminate\Support\Facades\Facade;

/**
 * Laravel facade for the DarshJDB client.
 *
 * Provides static-like access to the {@see \Darshjdb\Client} singleton.
 *
 * Usage:
 *   use Darshjdb\Laravel\Facade as DDB;
 *
 *   $posts = DDB::data('posts')->where('published', '=', true)->get();
 *   $user  = DDB::auth()->signIn('email@example.com', 'password');
 *   $url   = DDB::storage()->getUrl('/avatars/pic.jpg');
 *
 * @method static \Darshjdb\AuthClient    auth()
 * @method static \Darshjdb\QueryBuilder  data(string $entity)
 * @method static \Darshjdb\StorageClient storage()
 * @method static array                  query(array $query)
 * @method static array                  transact(array $ops)
 * @method static mixed                  fn(string $name, array $args = [])
 *
 * @see \Darshjdb\Client
 */
class Facade extends Facade
{
    /**
     * Get the registered name of the component.
     */
    protected static function getFacadeAccessor(): string
    {
        return 'darshjdb';
    }
}
