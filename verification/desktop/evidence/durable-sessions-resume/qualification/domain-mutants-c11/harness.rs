extern crate self as nessa_sdk;
extern crate current_sdk;
pub mod domain { pub mod agent_execution { pub mod sessions {
    pub use current_sdk::domain::agent_execution::sessions::ExecutionSessionId;
    #[path = "/workspace/scratch/durable-qualification/domain-mutants-c11/attachment.rs"] mod attachment;
    pub use attachment::*;
}}}
#[path = "/workspace/scratch/durable-qualification/domain-mutants-c11/provider_context.rs"] mod provider_context;
