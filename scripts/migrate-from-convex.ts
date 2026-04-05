#!/usr/bin/env npx tsx
/**
 * Convex-to-DarshanDB migration script.
 *
 * Reads exported Convex data (JSON) and writes it to a DarshanDB instance
 * via the REST API, converting each document field into a triple
 * (entity_id, attribute, value).
 *
 * Usage:
 *   npx tsx scripts/migrate-from-convex.ts \
 *     --input ./convex-export              \
 *     --url   http://localhost:7700        \
 *     --token YOUR_ACCESS_TOKEN
 *
 * The --input directory should contain one JSON file per Convex table,
 * e.g. `users.json`, `messages.json`. Each file is an array of documents.
 *
 * You can also point --input at a single JSON file that is a top-level
 * object keyed by table name:
 *   { "users": [...], "messages": [...] }
 *
 * @module migrate-from-convex
 */

import * as fs from 'node:fs';
import * as path from 'node:path';

/* -------------------------------------------------------------------------- */
/*  CLI argument parsing                                                       */
/* -------------------------------------------------------------------------- */

interface MigrateArgs {
  input: string;
  url: string;
  token: string;
  batchSize: number;
  dryRun: boolean;
}

function parseArgs(): MigrateArgs {
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

  if (!flags['input']) {
    console.error('Error: --input is required (path to Convex export directory or JSON file)');
    process.exit(1);
  }
  if (!flags['url'] && !flags['dry-run']) {
    console.error('Error: --url is required (DarshanDB server URL, e.g. http://localhost:7700)');
    process.exit(1);
  }

  return {
    input: flags['input']!,
    url: (flags['url'] ?? 'http://localhost:7700').replace(/\/+$/, ''),
    token: flags['token'] ?? '',
    batchSize: parseInt(flags['batch-size'] ?? '100', 10),
    dryRun: flags['dry-run'] === 'true',
  };
}

/* -------------------------------------------------------------------------- */
/*  Convex data loader                                                         */
/* -------------------------------------------------------------------------- */

interface ConvexDocument {
  _id: string;
  _creationTime?: number;
  [key: string]: unknown;
}

type TableData = Record<string, ConvexDocument[]>;

function loadConvexData(inputPath: string): TableData {
  const resolved = path.resolve(inputPath);
  const stat = fs.statSync(resolved);

  if (stat.isFile()) {
    const raw = JSON.parse(fs.readFileSync(resolved, 'utf-8'));

    // Single file may be { tableName: [...docs] } or just [...docs]
    if (Array.isArray(raw)) {
      const tableName = path.basename(resolved, path.extname(resolved));
      return { [tableName]: raw as ConvexDocument[] };
    }
    return raw as TableData;
  }

  if (stat.isDirectory()) {
    const tables: TableData = {};
    const files = fs.readdirSync(resolved).filter((f) => f.endsWith('.json'));

    for (const file of files) {
      const tableName = path.basename(file, '.json');
      const raw = JSON.parse(fs.readFileSync(path.join(resolved, file), 'utf-8'));
      tables[tableName] = Array.isArray(raw) ? (raw as ConvexDocument[]) : [];
    }

    return tables;
  }

  console.error(`Error: ${resolved} is neither a file nor a directory`);
  process.exit(1);
}

/* -------------------------------------------------------------------------- */
/*  Convert Convex documents to DarshanDB mutation ops                         */
/* -------------------------------------------------------------------------- */

interface MutateOp {
  entity: string;
  id: string;
  op: 'set';
  data: Record<string, unknown>;
}

/**
 * Convert a Convex document into a DarshanDB set operation.
 *
 * Convex internal fields are mapped:
 *   _id           -> used as entity id
 *   _creationTime -> :db/createdAt attribute
 *
 * A :db/type attribute is added with the table name so entities
 * can be queried by their original Convex table.
 */
function convexDocToOp(tableName: string, doc: ConvexDocument): MutateOp {
  const data: Record<string, unknown> = {
    ':db/type': tableName,
  };

  for (const [key, value] of Object.entries(doc)) {
    if (key === '_id') continue;
    if (key === '_creationTime') {
      data[':db/createdAt'] = value;
      continue;
    }
    data[key] = value;
  }

  return {
    entity: tableName,
    id: doc._id,
    op: 'set',
    data,
  };
}

/* -------------------------------------------------------------------------- */
/*  DarshanDB REST client                                                      */
/* -------------------------------------------------------------------------- */

async function sendBatch(
  url: string,
  token: string,
  ops: MutateOp[],
): Promise<{ tx: number }> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }

  const resp = await fetch(`${url}/api/mutate`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ ops }),
  });

  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`Mutation failed (${resp.status}): ${body}`);
  }

  return (await resp.json()) as { tx: number };
}

/* -------------------------------------------------------------------------- */
/*  Progress reporting                                                         */
/* -------------------------------------------------------------------------- */

function progressBar(current: number, total: number, width = 30): string {
  const pct = total === 0 ? 1 : current / total;
  const filled = Math.round(width * pct);
  const bar = '#'.repeat(filled) + '-'.repeat(width - filled);
  return `[${bar}] ${current}/${total} (${(pct * 100).toFixed(1)}%)`;
}

/* -------------------------------------------------------------------------- */
/*  Main                                                                       */
/* -------------------------------------------------------------------------- */

async function main(): Promise<void> {
  const config = parseArgs();

  console.log('=== Convex -> DarshanDB Migration ===\n');
  console.log(`  Input:      ${config.input}`);
  console.log(`  Server:     ${config.url}`);
  console.log(`  Batch size: ${config.batchSize}`);
  console.log(`  Dry run:    ${config.dryRun}\n`);

  // Load data
  const tables = loadConvexData(config.input);
  const tableNames = Object.keys(tables);

  if (tableNames.length === 0) {
    console.log('No tables found in export. Nothing to migrate.');
    return;
  }

  console.log(`Found ${tableNames.length} table(s): ${tableNames.join(', ')}\n`);

  let totalDocs = 0;
  let totalOps = 0;
  let totalTx = 0;

  for (const tableName of tableNames) {
    const docs = tables[tableName]!;
    console.log(`--- Table: ${tableName} (${docs.length} documents) ---`);

    if (docs.length === 0) {
      console.log('  (empty, skipping)\n');
      continue;
    }

    // Convert all docs to ops
    const ops = docs.map((doc) => convexDocToOp(tableName, doc));

    // Send in batches
    for (let i = 0; i < ops.length; i += config.batchSize) {
      const batch = ops.slice(i, i + config.batchSize);

      if (config.dryRun) {
        process.stdout.write(`  ${progressBar(Math.min(i + config.batchSize, ops.length), ops.length)} (dry run)\r`);
      } else {
        try {
          const result = await sendBatch(config.url, config.token, batch);
          totalTx = Math.max(totalTx, result.tx);
        } catch (err) {
          console.error(`\n  ERROR at batch ${Math.floor(i / config.batchSize) + 1}: ${err}`);
          console.error(`  Failed document IDs: ${batch.map((op) => op.id).join(', ')}`);
          process.exit(1);
        }
        process.stdout.write(`  ${progressBar(Math.min(i + config.batchSize, ops.length), ops.length)}\r`);
      }
    }

    process.stdout.write('\n');
    totalDocs += docs.length;
    totalOps += ops.length;
    console.log();
  }

  console.log('=== Migration Summary ===');
  console.log(`  Tables migrated: ${tableNames.length}`);
  console.log(`  Documents:       ${totalDocs}`);
  console.log(`  Operations:      ${totalOps}`);
  if (!config.dryRun) {
    console.log(`  Latest tx:       ${totalTx}`);
  }
  console.log('\nMigration complete.');
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
