//! Composition root — the single place that wires the running server.
//!
//! Like livelance's `CompositionRoot`: load config, build shared state, assemble
//! the Axum router, bind, and serve. No business rules live here; only dependency
//! wiring so `main` and tests have one entry point.
//!
//! ```text
//! Environment::from_system()
//!        │
//!        ▼
//! AppState::from_environment()
//!        │
//!        ▼
//! server::entrypoint::http::router(state)
//!        │
//!        ▼
//! TcpListener::bind → axum::serve
//! ```

mod root;

pub use root::CompositionRoot;
