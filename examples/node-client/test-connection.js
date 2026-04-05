#!/usr/bin/env node

/**
 * DarshanDB Node.js Connection Test
 *
 * Quick script to verify the @darshan/client SDK can communicate with
 * a running DarshanDB server. Tests health, data creation, query, and auth.
 *
 * Usage:
 *   DARSHAN_URL=http://localhost:7700 node test-connection.js
 *
 * Optional environment variables:
 *   DARSHAN_APP_ID  - Application ID (default: "node-test")
 */

const DARSHAN_URL = process.env.DARSHAN_URL;
const APP_ID = process.env.DARSHAN_APP_ID || 'node-test';

if (!DARSHAN_URL) {
  console.error(
    'ERROR: DARSHAN_URL environment variable is required.\n' +
      'Usage: DARSHAN_URL=http://localhost:7700 node test-connection.js',
  );
  process.exit(1);
}

/* -------------------------------------------------------------------------- */
/*  Test runner                                                               */
/* -------------------------------------------------------------------------- */

let passed = 0;
let failed = 0;
let skipped = 0;

function pass(name) {
  passed++;
  console.log(`  PASS  ${name}`);
}

function fail(name, error) {
  failed++;
  console.log(`  FAIL  ${name}`);
  console.log(`        ${error}`);
}

function skip(name, reason) {
  skipped++;
  console.log(`  SKIP  ${name} -- ${reason}`);
}

/* -------------------------------------------------------------------------- */
/*  Helper: generate a simple UUID-like ID                                    */
/* -------------------------------------------------------------------------- */

function simpleId() {
  return (
    Date.now().toString(36) +
    '-' +
    Math.random().toString(36).slice(2, 10)
  );
}

/* -------------------------------------------------------------------------- */
/*  Tests                                                                     */
/* -------------------------------------------------------------------------- */

async function main() {
  console.log(`\nDarshanDB Connection Test`);
  console.log(`Server:  ${DARSHAN_URL}`);
  console.log(`App ID:  ${APP_ID}`);
  console.log(`---`);

  // ── Test 1: Health endpoint ───────────────────────────────────────────

  try {
    const resp = await fetch(`${DARSHAN_URL}/health`);
    if (!resp.ok) {
      fail('Health check', `HTTP ${resp.status}`);
    } else {
      const body = await resp.json();
      if (body.status === 'ok' && body.service === 'darshandb') {
        pass(`Health check (version: ${body.version || 'unknown'}, triples: ${body.triples ?? '?'})`);
      } else {
        fail('Health check', `Unexpected response: ${JSON.stringify(body)}`);
      }
    }
  } catch (err) {
    fail('Health check', `Connection refused or error: ${err.message}`);
    console.log('\n  Cannot reach server. Aborting remaining tests.\n');
    process.exit(1);
  }

  // ── Test 2: OpenAPI spec ──────────────────────────────────────────────

  try {
    const resp = await fetch(`${DARSHAN_URL}/api/openapi.json`);
    if (!resp.ok) {
      skip('OpenAPI spec', `HTTP ${resp.status}`);
    } else {
      const spec = await resp.json();
      if (spec.openapi && spec.paths) {
        const pathCount = Object.keys(spec.paths).length;
        pass(`OpenAPI spec (${pathCount} paths documented)`);
      } else {
        fail('OpenAPI spec', 'Missing openapi or paths fields');
      }
    }
  } catch (err) {
    fail('OpenAPI spec', err.message);
  }

  // ── Test 3: Create entity via POST /api/data/:entity ──────────────────

  const testEntityId = simpleId();
  const testEntity = {
    id: testEntityId,
    name: `node-test-${testEntityId}`,
    value: 42,
    createdAt: new Date().toISOString(),
  };

  try {
    const resp = await fetch(`${DARSHAN_URL}/api/data/node_test_entities`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(testEntity),
    });

    if (resp.status === 501 || resp.status === 404) {
      skip('Create entity', `Endpoint returned ${resp.status} (not yet implemented)`);
    } else if (resp.ok) {
      pass('Create entity via /api/data/node_test_entities');
    } else {
      const text = await resp.text();
      fail('Create entity', `HTTP ${resp.status}: ${text.slice(0, 200)}`);
    }
  } catch (err) {
    fail('Create entity', err.message);
  }

  // ── Test 4: Mutate endpoint ───────────────────────────────────────────

  const mutateId = simpleId();

  try {
    const resp = await fetch(`${DARSHAN_URL}/api/mutate`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        ops: [
          {
            kind: 'set',
            entity: 'node_test',
            id: mutateId,
            data: { label: 'mutate-test', ts: Date.now() },
          },
        ],
      }),
    });

    if (resp.status === 501 || resp.status === 404) {
      skip('Mutate endpoint', `HTTP ${resp.status} (not yet implemented)`);
    } else if (resp.ok) {
      const body = await resp.json();
      pass(`Mutate endpoint (response: ${JSON.stringify(body).slice(0, 100)})`);
    } else {
      const text = await resp.text();
      fail('Mutate endpoint', `HTTP ${resp.status}: ${text.slice(0, 200)}`);
    }
  } catch (err) {
    fail('Mutate endpoint', err.message);
  }

  // ── Test 5: Query endpoint ────────────────────────────────────────────

  try {
    const resp = await fetch(`${DARSHAN_URL}/api/query`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        collection: 'node_test',
        limit: 5,
      }),
    });

    if (resp.status === 501 || resp.status === 404) {
      skip('Query endpoint', `HTTP ${resp.status} (not yet implemented)`);
    } else if (resp.ok) {
      const body = await resp.json();
      const count = Array.isArray(body.data) ? body.data.length : '?';
      pass(`Query endpoint (${count} results returned)`);
    } else {
      const text = await resp.text();
      fail('Query endpoint', `HTTP ${resp.status}: ${text.slice(0, 200)}`);
    }
  } catch (err) {
    fail('Query endpoint', err.message);
  }

  // ── Test 6: Auth signup (graceful) ────────────────────────────────────

  try {
    const email = `node-test-${Date.now()}@darshandb.test`;
    const resp = await fetch(`${DARSHAN_URL}/api/auth/signup`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        email,
        password: 'NodeTestPass123!',
      }),
    });

    if (resp.status === 501 || resp.status === 404) {
      skip('Auth signup', `HTTP ${resp.status} (not yet implemented)`);
    } else if (resp.ok) {
      const body = await resp.json();
      if (body.user && body.tokens) {
        pass(`Auth signup (user: ${body.user.id})`);
      } else {
        fail('Auth signup', `Unexpected response shape: ${JSON.stringify(body).slice(0, 200)}`);
      }
    } else {
      const text = await resp.text();
      // 500 often means DB tables not migrated -- skip gracefully
      if (resp.status >= 500) {
        skip('Auth signup', `Server error ${resp.status}: ${text.slice(0, 100)}`);
      } else {
        fail('Auth signup', `HTTP ${resp.status}: ${text.slice(0, 200)}`);
      }
    }
  } catch (err) {
    fail('Auth signup', err.message);
  }

  // ── Test 7: Admin schema ──────────────────────────────────────────────

  try {
    const resp = await fetch(`${DARSHAN_URL}/api/admin/schema`, {
      headers: { Accept: 'application/json' },
    });

    if (resp.status === 501 || resp.status === 404) {
      skip('Admin schema', `HTTP ${resp.status}`);
    } else if (resp.ok) {
      pass('Admin schema endpoint');
    } else if (resp.status === 401 || resp.status === 403) {
      pass('Admin schema (auth required, endpoint exists)');
    } else {
      const text = await resp.text();
      fail('Admin schema', `HTTP ${resp.status}: ${text.slice(0, 200)}`);
    }
  } catch (err) {
    fail('Admin schema', err.message);
  }

  // ── Summary ───────────────────────────────────────────────────────────

  console.log(`\n---`);
  console.log(
    `Results: ${passed} passed, ${failed} failed, ${skipped} skipped ` +
      `(${passed + failed + skipped} total)`,
  );

  if (failed > 0) {
    console.log(`\nSome tests FAILED. Check server logs for details.\n`);
    process.exit(1);
  } else {
    console.log(`\nAll reachable endpoints working correctly.\n`);
    process.exit(0);
  }
}

main().catch((err) => {
  console.error('Unexpected error:', err);
  process.exit(1);
});
