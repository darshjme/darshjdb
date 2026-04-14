// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root.
//
// In this worktree (Slice 9), the L1 DashMap implementation lives on a parallel
// branch (`feat/cache-l1-dashmap`, Slice 8). To let Slice 9 build and test in
// isolation we expose a minimal `l1_stub` module that mirrors the L1 surface
// the rest of the crate references. The final merge with Slice 8 will reconcile
// the two and replace `l1_stub` with the real `l1` module.

pub mod l1_stub;
pub mod l2;

pub use l1_stub as l1;
pub use l2::{L2Cache, L2Error, L2Result, StreamEntry};
