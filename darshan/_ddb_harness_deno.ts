#!/usr/bin/env -S deno run --allow-net --allow-read --allow-env --no-prompt
// DarshJDB Deno function execution harness.
//
// Usage: deno run [permissions] _ddb_harness_deno.ts <function-file> <export-name>
//
// Reads an ExecutionContext JSON from stdin, extracts `args`,
// dynamically imports the function file, calls the named export (or default),
// and writes an ExecutionResult JSON to stdout.
//
// Unlike the Node harness, Deno executes TypeScript natively -- no transpilation,
// no temporary files, no type-stripping hacks.

const functionFile = Deno.args[0];
const exportName = Deno.args[1] || "default";

if (!functionFile) {
  await Deno.stderr.write(
    new TextEncoder().encode(
      "Usage: _ddb_harness_deno.ts <function-file> [export-name]\n",
    ),
  );
  Deno.exit(1);
}

const startMs = Date.now();

try {
  // Read ExecutionContext from stdin.
  const chunks: Uint8Array[] = [];
  const reader = Deno.stdin.readable.getReader();

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }

  const input = new TextDecoder().decode(
    chunks.length === 1
      ? chunks[0]
      : await new Blob(chunks).arrayBuffer().then((b) => new Uint8Array(b)),
  );

  const context = JSON.parse(input);
  const args = context.args || {};

  // Resolve the function file to an absolute file:// URL for dynamic import.
  const resolvedPath = functionFile.startsWith("/")
    ? functionFile
    : `${Deno.cwd()}/${functionFile}`;
  const fileUrl = `file://${resolvedPath}`;

  // Dynamic import -- Deno handles .ts natively.
  const mod = await import(fileUrl);

  // Resolve the export.
  let fn: ((...a: unknown[]) => unknown) | undefined;
  if (exportName === "default") {
    fn = mod.default ?? (Object.keys(mod).length === 1 ? Object.values(mod)[0] as (...a: unknown[]) => unknown : undefined);
  } else {
    fn = mod[exportName];
  }

  if (typeof fn !== "function") {
    throw new Error(
      `Export "${exportName}" is not a function in ${functionFile}`,
    );
  }

  // Call the function.
  let result = fn(args);

  // Await if it returns a promise.
  if (result && typeof (result as Promise<unknown>).then === "function") {
    result = await result;
  }

  const durationMs = Date.now() - startMs;
  const output = JSON.stringify({
    value: result === undefined ? null : result,
    duration_ms: durationMs,
    peak_memory_bytes: null,
    logs: [],
  });

  await Deno.stdout.write(new TextEncoder().encode(output));
  Deno.exit(0);
} catch (err) {
  const msg =
    err instanceof Error ? err.stack || err.message : String(err);
  await Deno.stderr.write(new TextEncoder().encode(msg));
  Deno.exit(1);
}
