# Benchmarks

Run `cargo bench` in `packages/server/` to generate benchmarks.

We don't publish comparison numbers against other products because we haven't done controlled benchmarks yet. When we do, methodology and reproduction steps will be included here.

## Running benchmarks

```bash
cd packages/server
cargo bench
```

Results are written to `target/criterion/` with HTML reports.
