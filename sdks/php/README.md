# DarshJDB PHP SDK

Official PHP SDK for [DarshJDB](https://github.com/darshjdb/darshjdb) with first-class Laravel support.

## Requirements

- PHP 8.1+
- Guzzle 7.5+

## Installation

```bash
composer require darshan/darshan-php
```

## Quick Start (Plain PHP)

```php
use Darshjdb\Client;

$db = new Client([
    'serverUrl' => 'https://db.example.com',
    'apiKey'    => 'your-api-key',
]);

// Auth
$result = $db->auth()->signUp('alice@example.com', 'password123', [
    'displayName' => 'Alice',
]);
$user = $db->auth()->signIn('alice@example.com', 'password123');
$me   = $db->auth()->getUser();
$db->auth()->signOut();

// Query with fluent builder
$posts = $db->data('posts')
    ->where('published', '=', true)
    ->where('category', 'in', ['tech', 'design'])
    ->orderBy('createdAt', 'desc')
    ->limit(20)
    ->get();

// CRUD operations
$post = $db->data('posts')->create([
    'title' => 'Hello World',
    'body'  => 'My first post.',
]);
$db->data('posts')->update($post['id'], ['title' => 'Updated Title']);
$db->data('posts')->delete($post['id']);

// Raw DarshJQL query
$result = $db->query([
    'collection' => 'users',
    'where'      => [['field' => 'age', 'op' => '>=', 'value' => 18]],
    'order'      => [['field' => 'name', 'direction' => 'asc']],
    'limit'      => 50,
]);

// Transactions
$db->transact([
    ['kind' => 'set', 'entity' => 'accounts', 'id' => 'acc-1', 'data' => ['balance' => 900]],
    ['kind' => 'set', 'entity' => 'accounts', 'id' => 'acc-2', 'data' => ['balance' => 1100]],
]);

// Server-side functions
$summary = $db->fn('generateReport', ['month' => '2026-04']);

// File storage
$result = $db->storage()->upload('/avatars/photo.jpg', '/tmp/photo.jpg');
$url    = $db->storage()->getUrl('/avatars/photo.jpg');
$db->storage()->delete('/avatars/photo.jpg');
```

## Laravel Integration

### Setup

Laravel auto-discovers the service provider. Publish the config file:

```bash
php artisan vendor:publish --tag=ddb-config
```

Add to your `.env`:

```env
DDB_SERVER_URL=https://db.example.com
DDB_API_KEY=your-api-key
```

### Usage with Facade

```php
use Darshjdb\Laravel\Facade as Darshan;

// In a controller
class PostController extends Controller
{
    public function index()
    {
        $posts = Darshan::data('posts')
            ->where('published', '=', true)
            ->orderBy('createdAt', 'desc')
            ->limit(20)
            ->get();

        return view('posts.index', ['posts' => $posts['data']]);
    }

    public function store(Request $request)
    {
        $post = Darshan::data('posts')->create([
            'title' => $request->input('title'),
            'body'  => $request->input('body'),
        ]);

        return redirect()->route('posts.show', $post['id']);
    }
}
```

### Usage with Dependency Injection

```php
use Darshjdb\Client;

class UserService
{
    public function __construct(private Client $ddb)
    {
    }

    public function getActiveUsers(): array
    {
        $result = $this->ddb->data('users')
            ->where('active', '=', true)
            ->orderBy('lastSeen', 'desc')
            ->get();

        return $result['data'];
    }
}
```

### Auth in Middleware

```php
use Darshjdb\Client;

class DarshanAuthMiddleware
{
    public function __construct(private Client $ddb)
    {
    }

    public function handle($request, Closure $next)
    {
        $token = $request->bearerToken();
        if ($token) {
            $this->ddb->auth()->setToken($token);
        }

        return $next($request);
    }
}
```

## Error Handling

All SDK methods throw `Darshjdb\Exception` on failure:

```php
use Darshjdb\Exception;

try {
    $db->auth()->signIn('user@example.com', 'wrong-password');
} catch (Exception $e) {
    echo $e->getMessage();        // "invalid credentials"
    echo $e->getStatusCode();     // 401
    print_r($e->getErrorBody());  // Full server error payload
}
```

## Configuration

| Option      | Type   | Default | Description                      |
| ----------- | ------ | ------- | -------------------------------- |
| `serverUrl` | string | --      | DarshJDB server URL (required)  |
| `apiKey`    | string | --      | Application API key (required)   |
| `timeout`   | int    | 30      | HTTP timeout in seconds          |

## License

MIT
