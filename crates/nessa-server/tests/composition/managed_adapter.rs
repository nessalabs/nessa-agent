use super::*;
use nessa_sdk::application::agent_execution::providers::ExecutableUseError;
use std::sync::atomic::{AtomicUsize, Ordering};

struct NativeUse {
    path: PathBuf,
    admitted: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
}
struct Guard(Arc<AtomicUsize>);
impl ExecutableUseGuard for Guard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
impl ExecutableUse for NativeUse {
    fn executable(&self) -> &Path {
        &self.path
    }
    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        self.admitted.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Guard(self.released.clone())))
    }
}

#[test]
fn hosted_launch_and_override_share_the_exact_native_authority() {
    for (agent, variable) in [
        (AgentId::Claude, "CLAUDE_CODE_EXECUTABLE"),
        (AgentId::Codex, "CODEX_PATH"),
    ] {
        let admitted = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let path = PathBuf::from("/managed/native/executable");
        let native = ExecutableUseSnapshot::new(
            path.clone(),
            Arc::new(NativeUse {
                path: path.clone(),
                admitted: admitted.clone(),
                released: released.clone(),
            }),
        )
        .unwrap();
        let adapter: AgentRuntime = serde_json::from_value(serde_json::json!({ "command": "/bundle/node", "args": ["/bundle/adapter.js"], "model": "test", "toolsEnabled": false })).unwrap();
        let hosted = ManagedAdapter::new(agent, &adapter, native).unwrap();
        assert_eq!(
            hosted.runtime.command.executable(),
            Path::new("/bundle/node")
        );
        assert_eq!(
            hosted.environment.get(&OsString::from(variable)).unwrap(),
            path.as_os_str()
        );
        assert_eq!(admitted.load(Ordering::SeqCst), 0);
        let mut guard = hosted.runtime.command.admit().unwrap();
        assert_eq!(admitted.load(Ordering::SeqCst), 1);
        drop(hosted);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        guard.release().unwrap();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }
}
