#!/usr/bin/env npx tsx
/**
 * DarshanDB data export script.
 *
 * Reads all data from a DarshanDB instance via the REST API and exports
 * it as JSON files grouped by entity type.
 *
 * Usage:
 *   npx tsx scripts/export-darshandb.ts \
 *     --url    http://localhost:7700    \
 *     --token  YOUR_ACCESS_TOKEN       \
 *     --output ./export                \
 *     --tables users,messages
 *
 * Options:
 *   --url      DarshanDB server URL (required)
 *   --token    Access token for authentication
 *   --output   Output directory (default: ./darshandb-export)
 *   --tables   Comma-separated list of entity types to export (default: all)
 *   --pretty   Pretty-print JSON output (default: true)
 *
 * @module export-darshandb
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

/* -------------------------------------------------------------------------- */
/*  CLI argument parsing                                                       */
/* -------------------------------------------------------------------------- */

interface ExportArgs {
  url: string;
  token: string;
  output: string;
  tables: string[] | null;
  pretty: boolean;
}

function parseArgs(): ExportArgs {
  const args = process.argv.slice(2);
  const flags: Record<string, string> = {};

  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg.startsWith('--')) {
      const key = arg.slice(2);
      const next = args[i + 1];
      flags[key] = next && !next.startsWith('--') ? (i++, next) : 'true';
    }
  }

  if (!flags['url']) {
    console.error('Error: --url is required (DarshanDB server URL)');
    process.exit(1);
  }

  return {
    url: flags['url']!.replace(/\/+$/, ''),
    token: flags['token'] ?? '',
    output: flags['output'] ?? './darshandb-export',
    tables: flags['tables'] ? flags['tables'].split(',').map((t) => t.trim()) : null,
    pretty: flags['pretty'] !== 'false',
  };
}

/* -------------------------------------------------------------------------- */
/*  DarshanDB REST client                                                      */
/* -------------------------------------------------------------------------- */

async function queryDarshanDB(
  url: string,
  token: string,
  query: Record<string, unknown>,
): Promise<Record<string, unknown[]>> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const resp = await fetch(`${url}/api/query`, {
    method: 'POST',
    headers,
    body: JSON.stringify(query),
  });

  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`Query failed (${resp.status}): ${body}`);
  }

  return (await resp.json()) as Record<string, unknown[]>;
}

async function listEntities(
  url: string,
  token: string,
  tableName: string,
): Promise<unknown[]> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  // Use the REST-style CRUD endpoint to list all entities of a type
  const resp = await fetch(`${url}/api/data/${tableName}`, {
    method: 'GET',
    headers,
  });

  if (!resp.ok) {
    // Fallback: try DarshanQL query
    const queryResult = await queryDarshanDB(url, token, {
      [tableName]: {},
    });
    return queryResult[tableName] ?? [];
  }

  const body = (await resp.json()) as { data: unknown[]; total?: number; hasMore?: boolean };

  // Handle pagination if needed
  const allData = [...body.data];
  let hasMore = body.hasMore ?? false;
  let offset = body.data.length;

  while (hasMore) {
    const nextResp = await fetch(
      `${url}/api/data/${tableName}?offset=${offset}&limit=100`,
      { method: 'GET', headers },
    );
    if (!nextResp.ok) break;

    const nextBody = (await nextResp.json()) as { data: unknown[]; hasMore?: boolean };
    allData.push(...nextBody.data);
    hasMore = nextBody.hasMore ?? false;
    offset += nextBody.data.length;
  }

  return allData;
}

/* -------------------------------------------------------------------------- */
/*  Export logic                                                                */
/* -------------------------------------------------------------------------- */

async function discoverTables(url: string, token: string): Promise<string[]> {
  const headers: Record<string, string> = {};
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  // Try the admin health/info endpoint to discover tables
  try {
    const resp = await fetch(`${url}/api/admin/health`, { headers });
    if (resp.ok) {
      const body = (await resp.json()) as { tables?: string[] };
      if (body.tables) return body.tables;
    }
  } catch {
    // Admin endpoint may not be available
  }

  console.warn('Warning: Could not auto-discover tables. Use --tables to specify explicitly.');
  return [];
}

/* -------------------------------------------------------------------------- */
/*  Main                                                                       */
/* -------------------------------------------------------------------------- */

async function main(): Promise<void> {
  const config = parseArgs();

  console.log('=== DarshanDB Export ===\n');
  console.log(`  Server:  ${config.url}`);
  console.log(`  Output:  ${config.output}`);
  console.log(`  Tables:  ${config.tables ? config.tables.join(', ') : '(auto-discover)'}\n`);

  // Resolve tables
  let tables: string[];
  if (config.tables) {
    tables = config.tables;
  } else {
    tables = await discoverTables(config.url, config.token);
    if (tables.length === 0) {
      console.error('No tables found. Provide --tables explicitly.');
      process.exit(1);
    }
    console.log(`Discovered ${tables.length} table(s): ${tables.join(', ')}\n`);
  }

  // Create output directory
  const outputDir = path.resolve(config.output);
  fs.mkdirSync(outputDir, { recursive: true });

  let totalEntities = 0;

  for (const table of tables) {
    process.stdout.write(`  Exporting ${table}...`);

    try {
      const entities = await listEntities(config.url, config.token, table);
      const filePath = path.join(outputDir, `${table}.json`);
      const content = config.pretty
        ? JSON.stringify(entities, null, 2)
        : JSON.stringify(entities);

      fs.writeFileSync(filePath, content, 'utf-8');
      totalEntities += entities.length;
      console.log(` ${entities.length} entities -> ${table}.json`);
    } catch (err) {
      console.error(` FAILED: ${err}`);
    }
  }

  // Write a manifest
  const manifest = {
    exportedAt: new Date().toISOString(),
    server: config.url,
    tables: tables.map((t) => t),
    totalEntities,
  };
  fs.writeFileSync(
    path.join(outputDir, '_manifest.json'),
    JSON.stringify(manifest, null, 2),
    'utf-8',
  );

  console.log(`\n=== Export Summary ===`);
  console.log(`  Tables:   ${tables.length}`);
  console.log(`  Entities: ${totalEntities}`);
  console.log(`  Output:   ${outputDir}`);
  console.log('\nExport complete.');
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
