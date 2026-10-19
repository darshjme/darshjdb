import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

const manifest = JSON.parse(readFileSync(new URL('../package.json', import.meta.url)));
const [pack] = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json'], { encoding: 'utf8' }));
const files = new Set(pack.files.map(({ path }) => path));
for (const entry of [manifest.main, manifest.module, manifest.types, ...Object.values(manifest.exports['.'])]) {
  assert(files.has(entry.replace(/^\.\//, '')), `Published entry missing: ${entry}`);
}
assert.equal(Object.keys(manifest.exports['.'])[0], 'types');
execFileSync('tsc', ['--noEmit', '--strict', '--skipLibCheck', '--moduleResolution', 'bundler', '--module', 'ESNext', '--target', 'ES2022', 'tests/package-types.ts'], { stdio: 'inherit' });
console.log('Published entries and consumer declarations verified.');
