//! Identity-bearing tool observations and once-only permission requests.
//! Tools merge sparse observations; permission requests own resolution state.
mod permission_request;
mod tool_call;
pub use permission_request::PermissionRequest;
pub use tool_call::ToolCall;
