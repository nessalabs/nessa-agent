//! Infrastructure integration tests grouped by the boundary they exercise.
//! Model metadata tests parse external input; session storage tests verify leased
//! memory/file adapters, snapshot validation, and recovery without model calls.
mod model_metadata;
mod session_storage;
