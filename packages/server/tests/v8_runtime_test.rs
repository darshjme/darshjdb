//! VYASA — Embedded V8 isolate runtime integration test.
//!
//! DarshJDB — created by Darshankumar Joshi (github.com/darshjme).
//!
//! Gated on `#[cfg(feature = "v8")]` so `cargo test -p ddb-server` (default
//! features) skips this file entirely and nobody has to pay the 2-3 minute
//! V8 compile cost unless they asked for it.
//!
//! Run with:
//! ```bash
//! cargo test -p ddb-server --features v8 --test v8_runtime_test
//! ```

#![cfg(feature = "v8")]

use std::path::PathBuf;

use ddb_server::functions::V8Runtime;
use ddb_server::functions::registry::{FunctionDef, FunctionKind};
use ddb_server::functions::runtime::{ExecutionContext, ResourceLimits, RuntimeBackend};
use serde_json::json;
use tempfile::TempDir;

/// Smoke test: define a trivial ESM function that echoes its ctx back,
/// invoke it through the embedded V8 runtime, and assert the return value
/// matches what we passed in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v8_runtime_echoes_context() {
    // 1. Write a tiny user function file to a tempdir.
    let tmp = TempDir::new().expect("tempdir");
    let fn_path = tmp.path().join("hello.mjs");
    std::fs::write(
        &fn_path,
        // The function receives the full ExecutionContext. We reach into
        // ctx.args.name to prove the ctx round-tripped correctly.
        "export default (ctx) => ({ hello: ctx.args.name });",
    )
    .expect("write function file");

    // 2. Build the V8 runtime rooted at the tempdir.
    let runtime = V8Runtime::new(tmp.path().to_path_buf(), 4);

    // 3. Health check should succeed (V8 platform is initializable).
    runtime.health_check().await.expect("v8 health check");

    // 4. Assemble the FunctionDef + ExecutionContext + ResourceLimits.
    let func_def = FunctionDef {
        name: "test:hello".into(),
        file_path: PathBuf::from("hello.mjs"),
        export_name: "default".into(),
        kind: FunctionKind::Query,
        args_schema: None,
        description: None,
        last_modified: None,
    };

    let ctx = ExecutionContext {
        invocation_id: "test-inv-1".into(),
        function_name: "test:hello".into(),
        args: json!({ "name": "world" }),
        db_url: "postgres://unused/test".into(),
        auth_token: None,
        internal_api_url: "http://127.0.0.1:0".into(),
    };

    let limits = ResourceLimits::default();

    // 5. Execute and assert.
    let result = runtime
        .execute(&func_def, &ctx, &limits)
        .await
        .expect("v8 execute");

    assert_eq!(result.value, json!({ "hello": "world" }));
}
