use super::{before, Failure, MemoryState, NativeProcessMemory, ProcessMemory};
use std::{
    cell::{Cell, RefCell},
    process::Command,
};

const OPEN: MemoryState = MemoryState {
    core_soft: 1024,
    core_hard: 2048,
    dumpable: 1,
};
const PROTECTED: MemoryState = MemoryState {
    core_soft: 0,
    core_hard: 0,
    dumpable: 0,
};
struct Substitute {
    initial: MemoryState,
    confirmed: MemoryState,
    failure: Option<Failure>,
    verification_read_failure: Option<Failure>,
    reads: Cell<usize>,
    calls: RefCell<Vec<&'static str>>,
}
impl Substitute {
    fn new(initial: MemoryState, confirmed: MemoryState, failure: Option<Failure>) -> Self {
        Self {
            initial,
            confirmed,
            failure,
            verification_read_failure: None,
            reads: Cell::new(0),
            calls: RefCell::new(Vec::new()),
        }
    }
}
impl ProcessMemory for Substitute {
    fn state(&self) -> Result<MemoryState, Failure> {
        self.calls.borrow_mut().push("state");
        if matches!(
            self.failure,
            Some(Failure::ReadCoreLimit | Failure::ReadDumpability)
        ) {
            return Err(self.failure.unwrap());
        }
        let reads = self.reads.get();
        self.reads.set(reads + 1);
        if reads > 0 {
            if let Some(failure) = self.verification_read_failure {
                return Err(failure);
            }
        }
        Ok(if reads == 0 {
            self.initial
        } else {
            self.confirmed
        })
    }
    fn disable_core_dumps(&self) -> Result<(), Failure> {
        self.calls.borrow_mut().push("core");
        if self.failure == Some(Failure::SetCoreLimit) {
            Err(Failure::SetCoreLimit)
        } else {
            Ok(())
        }
    }
    fn disable_dumpability(&self) -> Result<(), Failure> {
        self.calls.borrow_mut().push("dumpability");
        if self.failure == Some(Failure::SetDumpability) {
            Err(Failure::SetDumpability)
        } else {
            Ok(())
        }
    }
}

#[test]
fn protections_are_verified_before_dispatch() {
    let process = Substitute::new(OPEN, PROTECTED, None);
    assert_eq!(
        before(&process, || {
            process.calls.borrow_mut().push("dispatch");
            7
        }),
        Ok(7)
    );
    assert_eq!(
        *process.calls.borrow(),
        ["state", "core", "dumpability", "state", "dispatch"]
    );
}

#[test]
fn already_protected_process_is_verified_without_changing_settings() {
    let process = Substitute::new(PROTECTED, PROTECTED, None);
    assert_eq!(before(&process, || 7), Ok(7));
    assert_eq!(*process.calls.borrow(), ["state", "state"]);
}

#[test]
fn each_missing_protection_is_set_independently() {
    let dumpable = Substitute::new(
        MemoryState {
            dumpable: 2,
            ..PROTECTED
        },
        PROTECTED,
        None,
    );
    assert_eq!(before(&dumpable, || ()), Ok(()));
    assert_eq!(*dumpable.calls.borrow(), ["state", "dumpability", "state"]);
    let core = Substitute::new(
        MemoryState {
            core_hard: 1,
            ..PROTECTED
        },
        PROTECTED,
        None,
    );
    assert_eq!(before(&core, || ()), Ok(()));
    assert_eq!(*core.calls.borrow(), ["state", "core", "state"]);
}

#[test]
fn refusals_do_not_dispatch() {
    for failure in [
        Failure::ReadCoreLimit,
        Failure::ReadDumpability,
        Failure::SetCoreLimit,
        Failure::SetDumpability,
    ] {
        let process = Substitute::new(OPEN, PROTECTED, Some(failure));
        let dispatched = Cell::new(false);
        let result = before(&process, || dispatched.set(true));
        assert_eq!(result, Err(failure));
        assert!(!dispatched.get());
        assert!(!process.calls.borrow().contains(&"dispatch"));
    }
}

#[test]
fn unreadable_verification_receipts_do_not_dispatch() {
    for failure in [Failure::ReadCoreLimit, Failure::ReadDumpability] {
        let mut process = Substitute::new(OPEN, PROTECTED, None);
        process.verification_read_failure = Some(failure);
        let dispatched = Cell::new(false);
        assert_eq!(before(&process, || dispatched.set(true)), Err(failure));
        assert!(!dispatched.get());
    }
}

#[test]
fn contradictory_receipts_do_not_dispatch() {
    for state in [
        MemoryState {
            dumpable: 1,
            ..PROTECTED
        },
        MemoryState {
            core_soft: 1,
            ..PROTECTED
        },
        MemoryState {
            core_hard: 1,
            ..PROTECTED
        },
    ] {
        let process = Substitute::new(OPEN, state, None);
        let dispatched = Cell::new(false);
        assert_eq!(
            before(&process, || dispatched.set(true)),
            Err(Failure::Verification)
        );
        assert!(!dispatched.get());
    }
}

#[test]
fn native_protection_changes_only_a_disposable_child() {
    let parent_before = NativeProcessMemory.state().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "core::process_memory::tests::native_child",
            "--ignored",
            "--nocapture",
        ])
        .env("NESSA_PROCESS_MEMORY_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line == "process-memory-protections-confirmed"));
    assert_eq!(NativeProcessMemory.state().unwrap(), parent_before);
}

#[test]
#[ignore = "only the parent test explicitly dispatches this in a disposable child"]
fn native_child() {
    assert_eq!(
        std::env::var("NESSA_PROCESS_MEMORY_TEST_CHILD").as_deref(),
        Ok("1")
    );
    before(&NativeProcessMemory, || {
        assert_eq!(NativeProcessMemory.state().unwrap(), PROTECTED);
    })
    .unwrap();
    // Applying it twice remains successful. No real key or secret is loaded.
    before(&NativeProcessMemory, || ()).unwrap();
    println!("process-memory-protections-confirmed");
}
