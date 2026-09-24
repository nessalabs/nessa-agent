//! Executable snapshots cannot detach a launch path from its use authority.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingUse {
    executable: PathBuf,
    admissions: Arc<AtomicUsize>,
}

impl ExecutableUse for CountingUse {
    fn executable(&self) -> &Path {
        &self.executable
    }

    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseError> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingGuard))
    }
}

struct CountingGuard;

impl ExecutableUseGuard for CountingGuard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        Ok(())
    }
}

#[test]
fn snapshot_rejects_a_path_other_than_the_authoritys_executable() {
    let admissions = Arc::new(AtomicUsize::new(0));
    let authority = Arc::new(CountingUse {
        executable: PathBuf::from("/managed/opencode"),
        admissions: admissions.clone(),
    });

    let result = ExecutableUseSnapshot::new(PathBuf::from("/substituted/opencode"), authority);

    assert_eq!(
        result.err(),
        Some(ExecutableUseError::new(
            "executable path does not match its use authority"
        ))
    );
    assert_eq!(admissions.load(Ordering::SeqCst), 0);
}

#[test]
fn each_launch_admission_receives_a_distinct_generation_guard() {
    let admissions = Arc::new(AtomicUsize::new(0));
    let authority = Arc::new(CountingUse {
        executable: PathBuf::from("/managed/opencode"),
        admissions: admissions.clone(),
    });
    let snapshot =
        ExecutableUseSnapshot::new(PathBuf::from("/managed/opencode"), authority).unwrap();

    let first = snapshot.admit().unwrap();
    let second = snapshot.admit().unwrap();

    assert_eq!(admissions.load(Ordering::SeqCst), 2);
    drop(first);
    drop(second);
    assert_eq!(admissions.load(Ordering::SeqCst), 2);
}
