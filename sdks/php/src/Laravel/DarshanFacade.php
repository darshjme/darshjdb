<?php

declare(strict_types=1);

namespace Darshan\Laravel;

use Illuminate\Support\Facades\Facade;

/**
 * Laravel facade for the DarshanDB client.
 *
 * Provides static-like access to the {@see \Darshan\Client} singleton.
 *
 * Usage:
 *   use Darshan\Laravel\DarshanFacade as Darshan;
 *
 *   $posts = Darshan::data('posts')->where('published', '=', true)->get();
 *   $user  = Darshan::auth()->signIn('email@example.com', 'password');
 *   $url   = Darshan::storage()->getUrl('/avatars/pic.jpg');
 *
 * @method static \Darshan\AuthClient    auth()
 * @method static \Darshan\QueryBuilder  data(string $entity)
 * @method static \Darshan\StorageClient storage()
 * @method static array                  query(array $query)
 * @method static array                  transact(array $ops)
 * @method static mixed                  fn(string $name, array $args = [])
 *
 * @see \Darshan\Client
 */
class DarshanFacade extends Facade
{
    /**
     * Get the registered name of the component.
     */
    protected static function getFacadeAccessor(): string
    {
        return 'darshan';
    }
}
