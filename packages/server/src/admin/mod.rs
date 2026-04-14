// Author: Darshankumar Joshi
//
// Admin dashboard subsystem.
//
// The DarshJDB admin dashboard (a Vite + React app at `packages/admin/`) is
// embedded directly into the `ddb-server` binary at compile time via the
// `include_dir!` macro. This means a single statically-linked binary ships
// the entire management UI — no separate web server, no `DDB_ADMIN_DIR`
// volume, no runtime asset hosting concerns.
//
// See [`static_assets`] for the file-serving handler and route wiring.

pub mod static_assets;
