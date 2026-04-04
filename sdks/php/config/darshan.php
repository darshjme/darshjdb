<?php

return [

    /*
    |--------------------------------------------------------------------------
    | DarshanDB Server URL
    |--------------------------------------------------------------------------
    |
    | The base URL of your DarshanDB server instance. All SDK requests
    | (queries, auth, storage) are sent to this endpoint.
    |
    */

    'server_url' => env('DARSHAN_SERVER_URL', 'http://localhost:6550'),

    /*
    |--------------------------------------------------------------------------
    | API Key
    |--------------------------------------------------------------------------
    |
    | Your application's API key, issued from the DarshanDB dashboard.
    | This authenticates your app (not individual users) with the server.
    |
    */

    'api_key' => env('DARSHAN_API_KEY', ''),

    /*
    |--------------------------------------------------------------------------
    | Request Timeout
    |--------------------------------------------------------------------------
    |
    | Maximum time in seconds to wait for a response from the DarshanDB
    | server before aborting the request.
    |
    */

    'timeout' => (int) env('DARSHAN_TIMEOUT', 30),

];
