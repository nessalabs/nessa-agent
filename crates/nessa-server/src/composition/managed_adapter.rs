//! A bundled Node adapter whose native agent is a managed installation.
//!
//! native snapshot -> environment override + hosted launch authority
//!                                      -> ACP process-tree cleanup
//! Arrows mean construction. The native snapshot keeps its original identity;
//! Node's authority admits use of that dependency before spawning the adapter.
use super::agent::AgentRuntime;
use crate::core::RunError;
use nessa_protocol::agents::AgentId;
use nessa_sdk::application::agent_execution::providers::{
    ExecutableUse, ExecutableUseAdmissionFailure, ExecutableUseGuard, ExecutableUseSnapshot,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The variable a managed adapter reads its native executable's location
/// from; `None` for an agent with no Node adapter. Where a program is, not
/// which context it keeps: composition launches it as runtime environment.
pub(super) fn native_locator(agent: AgentId) -> Option<&'static str> {
    match agent {
        AgentId::Claude => Some("CLAUDE_CODE_EXECUTABLE"),
        AgentId::Codex => Some("CODEX_PATH"),
        AgentId::Opencode => None,
    }
}

pub(super) struct ManagedAdapter {
    pub runtime: AgentRuntime,
    pub environment: BTreeMap<OsString, OsString>,
}

impl ManagedAdapter {
    pub fn new(
        agent: AgentId,
        adapter: &AgentRuntime,
        native: ExecutableUseSnapshot,
    ) -> Result<Self, RunError> {
        let Some(variable) = native_locator(agent) else {
            return Err(RunError::Agent("OpenCode has no Node adapter".into()));
        };
        let environment = BTreeMap::from([(
            OsString::from(variable),
            native.executable().as_os_str().to_owned(),
        )]);
        let node = adapter.command.executable().to_owned();
        let command = ExecutableUseSnapshot::new(
            node.clone(),
            Arc::new(AdapterExecutableUse { node, native }),
        )
        .map_err(|error| RunError::Agent(error.to_string()))?;
        Ok(Self {
            runtime: AgentRuntime {
                command,
                ..adapter.clone()
            },
            environment,
        })
    }
}

struct AdapterExecutableUse {
    node: PathBuf,
    native: ExecutableUseSnapshot,
}
impl ExecutableUse for AdapterExecutableUse {
    fn executable(&self) -> &Path {
        &self.node
    }
    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        self.native.admit()
    }
}

#[cfg(test)]
#[path = "../../tests/composition/managed_adapter.rs"]
mod tests;
