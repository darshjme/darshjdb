# PHP SDK

The `darshan/db` PHP package provides a Guzzle-based client for DarshJDB with a fluent query builder, authentication, file storage, and Laravel integration.

## Installation

```bash
composer require darshan/db
```

Requires PHP 8.1+ and `guzzlehttp/guzzle`.

## Client Setup

```php
use Darshan\Client;

$db = new Client([
    'serverUrl' => 'https://db.example.com',
    'apiKey'    => 'your-api-key',
    'timeout'   => 30, // seconds (optional, default: 30)
]);
```

Both `serverUrl` and `apiKey` are required. The API key is sent as the `X-Api-Key` header on every request.

## Authentication

Access the auth client via `$db->auth()`.

### Sign Up

```php
$result = $db->auth()->signUp('alice@example.com', 'strongpassword', [
    'displayName' => 'Alice',
]);
// $result = ['user' => [...], 'accessToken' => '...', 'refreshToken' => '...']
```

### Sign In

```php
$result = $db->auth()->signIn('alice@example.com', 'strongpassword');
echo $result['user']['id'];
```

The access token is automatically set on the client after sign-in.

### OAuth

```php
$result = $db->auth()->signInWithOAuth('google', $oauthToken, $redirectUri);
```

Supported providers: `google`, `github`, `apple`, `discord`.

### Sign Out

```php
$db->auth()->signOut();
// Token is cleared from the client
```

### Get Current User

```php
$user = $db->auth()->getUser();
// Returns null if not authenticated (401)
```

### Token Refresh

```php
$tokens = $db->auth()->refresh($refreshToken);
// $tokens = ['accessToken' => '...', 'refreshToken' => '...', 'expiresAt' => ...]
```

### Manual Token

Restore a session from a stored token:

```php
$db->auth()->setToken('eyJhbGciOiJIUzI1NiIs...');
```

## Fluent Query Builder

Start a query with `$db->data('collection')`. The builder supports chaining for filters, ordering, pagination, and field selection.

### Read

```php
$posts = $db->data('posts')
    ->where('published', '=', true)
    ->where('category', 'in', ['tech', 'design'])
    ->orderBy('createdAt', 'desc')
    ->limit(20)
    ->offset(40)
    ->select(['id', 'title', 'summary'])
    ->get();

// $posts = ['data' => [...], 'txId' => '...']
```

### Supported Operators

`=`, `!=`, `>`, `>=`, `<`, `<=`, `in`, `not-in`, `contains`, `starts-with`

### Create

```php
$post = $db->data('posts')->create([
    'title' => 'Hello World',
    'body'  => 'First post content.',
]);
// Returns the created record with server-assigned ID
```

### Update

```php
$db->data('posts')->update('post-id', [
    'title' => 'Updated Title',
]);
```

Uses merge semantics -- only the specified fields are changed.

### Delete

```php
$db->data('posts')->delete('post-id');
```

## Raw Queries

Execute a DarshJQL query descriptor directly:

```php
$result = $db->query([
    'collection' => 'users',
    'where' => [
        ['field' => 'age', 'op' => '>=', 'value' => 18],
    ],
    'order' => [
        ['field' => 'name', 'direction' => 'asc'],
    ],
    'limit' => 50,
]);
```

## Transactions

Execute multiple operations atomically:

```php
$result = $db->transact([
    ['kind' => 'set',    'entity' => 'users', 'id' => 'user-1', 'data' => ['name' => 'Alice']],
    ['kind' => 'merge',  'entity' => 'posts', 'id' => 'post-1', 'data' => ['views' => 100]],
    ['kind' => 'delete', 'entity' => 'comments', 'id' => 'c-1'],
]);
// $result = ['txId' => '...']
```

## Server Functions

Call registered server-side functions:

```php
$report = $db->fn('generateReport', ['month' => 3, 'year' => 2026]);
```

## File Storage

Access the storage client via `$db->storage()`.

### Upload

```php
$result = $db->storage()->upload('/avatars/photo.jpg', $fileContents, 'photo.jpg', [
    'contentType' => 'image/jpeg',
]);
// $result = ['path' => '...', 'url' => '...', 'size' => ...]
```

### Get URL

```php
$url = $db->storage()->getUrl('/avatars/photo.jpg');
```

### Delete

```php
$db->storage()->delete('/avatars/photo.jpg');
```

## Error Handling

All API errors throw `DarshanException`:

```php
use Darshan\DarshanException;

try {
    $db->auth()->signIn('bad@email.com', 'wrong');
} catch (DarshanException $e) {
    echo $e->getMessage();
    echo $e->getStatusCode(); // 401
}
```

`DarshanException::fromGuzzle()` wraps Guzzle exceptions with the server's error message and status code intact.

## Laravel Integration

### Service Provider

Register DarshJDB in your Laravel application:

```php
// config/darshandb.php
return [
    'server_url' => env('DARSHANDB_URL', 'http://localhost:7700'),
    'api_key'    => env('DARSHANDB_API_KEY'),
    'timeout'    => env('DARSHANDB_TIMEOUT', 30),
];
```

```php
// app/Providers/DarshanServiceProvider.php
namespace App\Providers;

use Darshan\Client;
use Illuminate\Support\ServiceProvider;

class DarshanServiceProvider extends ServiceProvider
{
    public function register(): void
    {
        $this->app->singleton(Client::class, function ($app) {
            return new Client([
                'serverUrl' => config('darshandb.server_url'),
                'apiKey'    => config('darshandb.api_key'),
                'timeout'   => config('darshandb.timeout'),
            ]);
        });
    }
}
```

### Facade

```php
// app/Facades/DarshanDB.php
namespace App\Facades;

use Darshan\Client;
use Illuminate\Support\Facades\Facade;

class DarshanDB extends Facade
{
    protected static function getFacadeAccessor(): string
    {
        return Client::class;
    }
}
```

### Usage in Controllers

```php
use App\Facades\DarshanDB;

class UserController extends Controller
{
    public function index()
    {
        $users = DarshanDB::data('users')
            ->where('active', '=', true)
            ->orderBy('createdAt', 'desc')
            ->limit(25)
            ->get();

        return response()->json($users);
    }

    public function store(Request $request)
    {
        $user = DarshanDB::data('users')->create([
            'name'  => $request->input('name'),
            'email' => $request->input('email'),
        ]);

        return response()->json($user, 201);
    }
}
```

## curl Examples

### Query

```bash
curl -X POST https://db.example.com/api/query \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: your-api-key" \
  -d '{"collection":"users","where":[{"field":"age","op":">=","value":18}],"limit":10}'
```

### Create

```bash
curl -X POST https://db.example.com/api/data/users \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: your-api-key" \
  -d '{"name":"Alice","email":"alice@example.com"}'
```

### Sign In

```bash
curl -X POST https://db.example.com/api/auth/signin \
  -H "Content-Type: application/json" \
  -d '{"email":"alice@example.com","password":"s3cret"}'
```
