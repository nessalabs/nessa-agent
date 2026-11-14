//! Server health — liveness for probes and connected clients.
//!
//! Two surfaces: HTTP `GET /health` for load balancers/smoke tests, and
//! `server.health` RPC for `@nessa/client`. Both report runtime readiness;
//! the RPC includes uptime from `AppState`.
//!
//! ```text
//! GET /health ──────────────► entrypoint::handle_http_health  → 200 OK
//!
//! server.health (WS, post-connect)
//!        │
//!        ▼
//! entrypoint::handle_health_check ──► HealthResult
//! ```

pub mod entrypoint;
