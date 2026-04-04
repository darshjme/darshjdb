<?php

declare(strict_types=1);

namespace Darshan\Laravel;

use Darshan\Client;
use Illuminate\Support\ServiceProvider;

/**
 * Laravel service provider for DarshanDB.
 *
 * Registers the DarshanDB {@see Client} as a singleton in the service container
 * and publishes the configuration file.
 *
 * Register in config/app.php or rely on Laravel auto-discovery:
 *   'providers' => [
 *       Darshan\Laravel\DarshanServiceProvider::class,
 *   ],
 */
class DarshanServiceProvider extends ServiceProvider
{
    /**
     * Register the DarshanDB client singleton.
     */
    public function register(): void
    {
        $this->mergeConfigFrom(
            __DIR__ . '/../../config/darshan.php',
            'darshan',
        );

        $this->app->singleton(Client::class, function ($app) {
            /** @var array{server_url: string, api_key: string, timeout: int} $config */
            $config = $app['config']['darshan'];

            return new Client([
                'serverUrl' => $config['server_url'],
                'apiKey'    => $config['api_key'],
                'timeout'   => $config['timeout'] ?? 30,
            ]);
        });

        $this->app->alias(Client::class, 'darshan');
    }

    /**
     * Bootstrap package services.
     */
    public function boot(): void
    {
        if ($this->app->runningInConsole()) {
            $this->publishes([
                __DIR__ . '/../../config/darshan.php' => config_path('darshan.php'),
            ], 'darshan-config');
        }
    }

    /**
     * Get the services provided by the provider.
     *
     * @return string[]
     */
    public function provides(): array
    {
        return [Client::class, 'darshan'];
    }
}
