//! # primer-storage
//!
//! SQLite-backed implementations of the persistence traits defined in
//! `primer-core::storage`.
//!
//! Mirrors the locking and error patterns of `primer-knowledge`: a
//! single `Connection` wrapped in `Mutex`, async trait methods with
//! synchronous bodies (acceptable at our turn rate; revisit if profiling
//! ever shows contention).
//!
//! ## Concurrency caveat
//!
//! The lock is `std::sync::Mutex`, taken from inside an async fn. On a
//! slow disk that means we block the tokio runtime while the SQLite
//! write completes. Acceptable for a single-user CLI; if a future
//! deployment ever has multiple concurrent writers (parallel learners
//! sharing a runtime, or a multi-process consumer), revisit with a
//! `tokio::sync::Mutex` and/or `spawn_blocking`.

mod catalog;
mod schema;
mod store;

pub use store::SqliteSessionStore;

// `#[doc(hidden)]` test-only re-export. See the function's own doc for
// the contract — production code must not call this.
#[doc(hidden)]
pub use store::__session_store_open_count_for_tests;
