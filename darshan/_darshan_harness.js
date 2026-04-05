#!/usr/bin/env node
// DarshJDB function execution harness.
//
// Usage: node _darshan_harness.js <function-file> <export-name>
//
// Reads an ExecutionContext JSON from stdin, extracts `args`,
// requires the function file, calls the named export (or default),
// and writes an ExecutionResult JSON to stdout.

"use strict";

const fs = require("fs");
const path = require("path");

const functionFile = process.argv[2];
const exportName = process.argv[3] || "default";

if (!functionFile) {
  process.stderr.write("Usage: _darshan_harness.js <function-file> [export-name]\n");
  process.exit(1);
}

// Read ExecutionContext from stdin.
let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => { input += chunk; });
process.stdin.on("end", async () => {
  const startMs = Date.now();
  try {
    const context = JSON.parse(input);
    const args = context.args || {};

    // Resolve the function file path.
    const resolved = path.resolve(functionFile);

    // For TypeScript files, use a simple require approach.
    // If the file is .ts, we transpile on the fly or fall back to
    // stripping types (works for simple TS without advanced features).
    let mod;
    const ext = path.extname(resolved);
    if (ext === ".ts" || ext === ".mts") {
      // Read file, strip type annotations naively for simple cases,
      // then evaluate. For production, use tsx or ts-node.
      const source = fs.readFileSync(resolved, "utf8");
      const stripped = stripSimpleTypes(source);
      // Use a temporary .js file approach via Function constructor
      const tmpFile = resolved.replace(/\.m?ts$/, ".__darshan_tmp.js");
      // Convert ESM export default to module.exports
      const cjsSource = esmToCjs(stripped);
      fs.writeFileSync(tmpFile, cjsSource, "utf8");
      try {
        mod = require(tmpFile);
      } finally {
        try { fs.unlinkSync(tmpFile); } catch (_) {}
      }
    } else {
      mod = require(resolved);
    }

    // Resolve the export.
    let fn;
    if (exportName === "default") {
      fn = mod.default || mod;
    } else {
      fn = mod[exportName];
    }

    if (typeof fn !== "function") {
      throw new Error(`Export "${exportName}" is not a function in ${functionFile}`);
    }

    // Call the function.
    let result = fn(args);

    // Await if it returns a promise.
    if (result && typeof result.then === "function") {
      result = await result;
    }

    const durationMs = Date.now() - startMs;
    const output = JSON.stringify({
      value: result === undefined ? null : result,
      duration_ms: durationMs,
      peak_memory_bytes: null,
      logs: [],
    });

    process.stdout.write(output);
    process.exit(0);
  } catch (err) {
    process.stderr.write(err.stack || err.message || String(err));
    process.exit(1);
  }
});

// Naive type-stripping for simple TypeScript (handles common patterns).
function stripSimpleTypes(source) {
  let result = source;
  // Remove type annotations on parameters: (x: Type) -> (x)
  result = result.replace(/:\s*\{[^}]*\}/g, "");
  // Remove simple type annotations: : string, : number, etc.
  result = result.replace(/:\s*(?:string|number|boolean|any|void|never|null|undefined|unknown)(?:\[\])?\s*(?=[,)=;{\n])/g, "");
  // Remove interface/type declarations (single line)
  result = result.replace(/^(?:export\s+)?(?:interface|type)\s+\w+.*$/gm, "");
  // Remove generic type params on functions: <T> -> empty
  result = result.replace(/<[A-Z][^>]*>/g, "");
  return result;
}

// Convert ESM-style exports to CJS.
function esmToCjs(source) {
  let result = source;
  // export default function NAME(...) -> module.exports.default = function NAME(...)
  result = result.replace(
    /export\s+default\s+function\s+(\w+)/g,
    "module.exports.default = function $1"
  );
  // export default function(...) -> module.exports.default = function(...)
  result = result.replace(
    /export\s+default\s+function\s*\(/g,
    "module.exports.default = function("
  );
  // export default expr -> module.exports.default = expr
  result = result.replace(/export\s+default\s+/g, "module.exports.default = ");
  // export const NAME = -> module.exports.NAME =
  result = result.replace(/export\s+const\s+(\w+)\s*=/g, "module.exports.$1 =");
  // export function NAME -> module.exports.NAME = function NAME
  result = result.replace(/export\s+function\s+(\w+)/g, "module.exports.$1 = function $1");
  // Remove import statements (for now, functions are self-contained)
  result = result.replace(/^import\s+.*$/gm, "");
  return result;
}
