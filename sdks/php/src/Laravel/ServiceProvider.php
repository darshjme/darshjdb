<?php

declare(strict_types=1);

namespace Darshjdb\Laravel;

use Darshjdb\Client;
use Illuminate\Support\ServiceProvider;

/**
 * Laravel service provider for DarshJDB.
 *
 * Registers the DarshJDB {@see Client} as a singleton in the service container
 * and publishes the configuration file.
 *
 * Register in config/app.php or rely on Laravel auto-discovery:
 *   'providers' => [
 *       Darshjdb\Laravel\ServiceProvider::class,
 *   ],
 */
class ServiceProvider extends ServiceProvider
{
    /**
     * Register the DarshJDB client singleton.
     */
    public function register(): void
    {
        $this->mergeConfigFrom(
            __DIR__ . '/../../config/darshjdb.php',
            'darshjdb',
        );

        $this->app->singleton(Client::class, function ($app) {
            /** @var array{server_url: string, api_key: string, timeout: int} $config */
            $config = $app['config']['darshjdb'];

            return new Client([
                'serverUrl' => $config['server_url'],
                'apiKey'    => $config['api_key'],
                'timeout'   => $config['timeout'] ?? 30,
            ]);
        });

        $this->app->alias(Client::class, 'darshjdb');
    }

    /**
     * Bootstrap package services.
     */
    public function boot(): void
    {
        if ($this->app->runningInConsole()) {
            $this->publishes([
                __DIR__ . '/../../config/darshjdb.php' => config_path("darshjdb.php"),
            ], 'darshjdb-config');
        }
    }

    /**
     * Get the services provided by the provider.
     *
     * @return string[]
     */
    public function provides(): array
    {
        return [Client::class, 'darshjdb'];
    }
}
