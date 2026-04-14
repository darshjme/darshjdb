<?php

return [

    /*
    |--------------------------------------------------------------------------
    | DarshJDB Server URL
    |--------------------------------------------------------------------------
    |
    | The base URL of your DarshJDB server instance. All SDK requests
    | (queries, auth, storage) are sent to this endpoint.
    |
    */

    'server_url' => env('DDB_SERVER_URL', 'http://localhost:6550'),

    /*
    |--------------------------------------------------------------------------
    | API Key
    |--------------------------------------------------------------------------
    |
    | Your application's API key, issued from the DarshJDB dashboard.
    | This authenticates your app (not individual users) with the server.
    |
    */

    'api_key' => env('DDB_API_KEY', ''),

    /*
    |--------------------------------------------------------------------------
    | Request Timeout
    |--------------------------------------------------------------------------
    |
    | Maximum time in seconds to wait for a response from the DarshJDB
    | server before aborting the request.
    |
    */

    'timeout' => (int) env('DDB_TIMEOUT', 30),

];
