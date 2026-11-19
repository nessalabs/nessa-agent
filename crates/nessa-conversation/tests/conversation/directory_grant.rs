#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use super::*;
use crate::domain::{PathLeave, SandboxKind, WalkFacts, BINDING_LEN, MAX_ENCODED_GRANT_BYTES};
use crate::infrastructure::{
    CedarDirectoryPolicyEvaluator, LocalDirectoryFiles, SqliteDirectoryStore,
};

fn private() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

struct FixedBinding([u8; BINDING_LEN]);

impl BindingSource for FixedBinding {
    fn mint(&self) -> [u8; BINDING_LEN] {
        self.0
    }
}

struct RecordingAudit {
    lines: Mutex<Vec<DirectoryAuditRecord>>,
    fail: Mutex<bool>,
}

impl RecordingAudit {
    fn new() -> Self {
        Self {
            lines: Mutex::new(Vec::new()),
            fail: Mutex::new(false),
        }
    }
}

impl DirectoryAudit for RecordingAudit {
    fn record(&self, record: &DirectoryAuditRecord) -> Result<(), AuditUnavailable> {
        if *self.fail.lock().expect("fail") {
            return Err(AuditUnavailable);
        }
        self.lines.lock().expect("lines").push(record.clone());
        Ok(())
    }
}

fn service(
    files: Arc<dyn DirectoryFiles>,
    store: Arc<dyn DirectoryStore>,
    audit: Arc<dyn DirectoryAudit>,
    binding: [u8; BINDING_LEN],
) -> DirectoryAccess {
    DirectoryAccess::new(
        files,
        store,
        Arc::new(CedarDirectoryPolicyEvaluator::new().unwrap()),
        audit,
        Arc::new(FixedBinding(binding)),
    )
}

fn share_read(access: &DirectoryAccess, path: &str, subject: &str) -> SharedDirectory {
    access
        .share(ShareDirectory {
            environment: "local".to_owned(),
            path: path.to_owned(),
            subject: DirectorySubject {
                principal_id: Some(subject.to_owned()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names([
                "read", "write", "exec",
            ])
            .unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".to_owned(),
        })
        .unwrap()
}

#[test]
fn a_write_creates_a_file_under_a_directory_inside_the_share() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [24; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    fs::create_dir(shared.join("sub")).unwrap();
    fs::write(shared.join("sub/existing"), b"old").unwrap();
    access.write("peer", "sub/existing", b"nested").unwrap();
    assert_eq!(access.read("peer", "sub/existing").unwrap(), b"nested");
}

#[test]
fn a_peer_reads_a_file_inside_and_not_a_path_that_leaves() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("note.txt"), b"hello").unwrap();
    let secret = root.path().join("secret.txt");
    fs::write(&secret, b"secret-bytes").unwrap();
    std::os::unix::fs::symlink(&secret, shared.join("linked")).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [7; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert_eq!(access.read("peer", "note.txt").unwrap(), b"hello");
    let left = access.read("peer", "linked").unwrap_err();
    assert!(matches!(
        left,
        DirectoryRefusal::Outside {
            leave: PathLeave::Symlink
        }
    ));
    assert!(!left.to_string().contains("secret"));
    let dotdot = access.read("peer", "sub/../../secret.txt").unwrap_err();
    assert!(matches!(dotdot, DirectoryRefusal::Outside { .. }));
    assert!(!dotdot.to_string().contains("secret"));
}

#[test]
fn the_marker_and_a_hard_link_of_it_are_refused() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [3; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    fs::hard_link(shared.join(".nessa-binding"), shared.join("alias")).unwrap();
    assert!(matches!(
        access.read("peer", ".nessa-binding").unwrap_err(),
        DirectoryRefusal::MarkerName
    ));
    assert!(matches!(
        access.read("peer", "alias").unwrap_err(),
        DirectoryRefusal::MarkerName
    ));
    let before = fs::read(shared.join(".nessa-binding")).unwrap();
    assert!(matches!(
        access.write("peer", ".nessa-binding", b"nope").unwrap_err(),
        DirectoryRefusal::MarkerName
    ));
    assert_eq!(fs::read(shared.join(".nessa-binding")).unwrap(), before);
}

#[test]
fn a_case_folded_marker_name_is_the_marker_when_the_disk_folds_case() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join(".NESSA-BINDING"), b"other").unwrap();
    let folds =
        fs::read(shared.join(".nessa-binding")).ok().as_deref() == Some(b"other".as_slice());
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [4; BINDING_LEN],
    );
    let shared_path = shared.to_str().unwrap();
    if folds {
        let refused = access
            .share(ShareDirectory {
                environment: "local".into(),
                path: shared_path.into(),
                subject: DirectorySubject {
                    principal_id: Some("peer".into()),
                    receiver_id: None,
                },
                capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
                rules: crate::domain::CommandRules::none(),
                initiator: "owner".into(),
            })
            .unwrap_err();
        assert!(matches!(refused, DirectoryRefusal::MarkerSize));
        assert_eq!(fs::read(shared.join(".nessa-binding")).unwrap(), b"other");
    } else {
        share_read(&access, shared_path, "peer");
        assert_eq!(access.read("peer", ".NESSA-BINDING").unwrap(), b"other");
    }
}

#[test]
fn deny_wins_and_an_unlisted_program_is_refused() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        audit.clone(),
        [5; BINDING_LEN],
    );
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["exec"]).unwrap(),
            rules: crate::domain::CommandRules::new(vec![
                allow_program("ls").unwrap(),
                allow_program("rm").unwrap(),
                deny_argv("rm", "-*").unwrap(),
            ])
            .unwrap(),
            initiator: "owner".into(),
        })
        .unwrap();
    assert!(access.command_at("peer", "ls", &[]).is_ok());
    let refused = access
        .command_at("peer", "/bin/rm", &["-rf".into()])
        .unwrap_err();
    match refused {
        DirectoryRefusal::Program { matcher, .. } => assert_eq!(matcher, "command.rules.deny.2"),
        other => panic!("{other:?}"),
    }
    let unlisted = access.command_at("peer", "cat", &[]).unwrap_err();
    match unlisted {
        DirectoryRefusal::Program { matcher, .. } => assert_eq!(matcher, "command.rules.unlisted"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn exec_names_the_sandbox_fact_that_refused_it() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [6; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    let none = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::None,
            roots: std::slice::from_ref(&shared.to_str().unwrap().to_owned()),
            hooks_answer_first: false,
        })
        .unwrap_err();
    match none {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|id| id == "directory.exec_none"))
        }
        other => panic!("{other:?}"),
    }
    let outside = root.path().join("elsewhere");
    fs::create_dir(&outside).unwrap();
    let root_outside = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Container,
            roots: &[outside.to_str().unwrap().to_owned()],
            hooks_answer_first: false,
        })
        .unwrap_err();
    match root_outside {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|id| id == "directory.exec_root"))
        }
        other => panic!("{other:?}"),
    }
    let directory = shared.to_str().unwrap().to_owned();
    let hidden = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Container,
            roots: std::slice::from_ref(&directory),
            hooks_answer_first: false,
        })
        .unwrap_err();
    match hidden {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|id| id == "directory.exec_marker"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn hooks_that_answer_first_do_not_skip_command_rules() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [8; BINDING_LEN],
    );
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["exec"]).unwrap(),
            rules: crate::domain::CommandRules::new(vec![allow_program("ls").unwrap()]).unwrap(),
            initiator: "owner".into(),
        })
        .unwrap();
    let refused = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Harness,
            roots: &[shared.to_str().unwrap().to_owned()],
            hooks_answer_first: true,
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::HooksAnswerFirst));
}

#[test]
fn an_unasked_call_is_recorded_outside_nessas_gate() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        audit.clone(),
        [9; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    access.report_unasked("peer", "Bash").unwrap();
    let lines = audit.lines.lock().unwrap();
    assert!(lines.iter().any(|line| line.cause == "outside-nessa-gate"));
}

#[test]
fn sharing_writes_the_marker_and_not_harness_settings() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [1; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert!(shared.join(".nessa-binding").is_file());
    assert!(!shared.join(".claude").exists());
    assert!(!shared.join(".codex").exists());
}

#[test]
fn an_orphan_marker_of_32_bytes_is_adopted_and_a_longer_one_is_not_rewritten() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let bytes = [2u8; BINDING_LEN];
    fs::write(shared.join(".nessa-binding"), bytes).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [9; BINDING_LEN],
    );
    let shared_grant = share_read(&access, shared.to_str().unwrap(), "peer");
    assert_eq!(fs::read(shared.join(".nessa-binding")).unwrap(), bytes);
    assert!(!shared_grant.resource_id.is_empty());

    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join(".nessa-binding"), vec![0; 33]).unwrap();
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: other.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("second".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::MarkerSize));
    assert_eq!(fs::read(other.join(".nessa-binding")).unwrap().len(), 33);
}

#[test]
fn a_second_subject_reuses_the_resource_and_a_copied_marker_is_refused() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [4; BINDING_LEN],
    );
    let first = share_read(&access, shared.to_str().unwrap(), "peer");
    let second = share_read(&access, shared.to_str().unwrap(), "other");
    assert_eq!(first.resource_id, second.resource_id);
    let copy = root.path().join("copy");
    fs::create_dir(&copy).unwrap();
    fs::copy(shared.join(".nessa-binding"), copy.join(".nessa-binding")).unwrap();
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: copy.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("thief".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::BindingCopied));
}

#[test]
fn recreating_the_directory_without_the_marker_revokes_the_grant() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        audit,
        [5; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_dir_all(&shared).unwrap();
    fs::create_dir(&shared).unwrap();
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { cause, .. } => assert_eq!(cause, "marker-missing"),
        other => panic!("{other:?}"),
    }
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
}

#[test]
fn a_failed_audit_still_revokes_and_a_retry_keeps_the_original_initiator() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        audit.clone(),
        [6; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    *audit.fail.lock().unwrap() = true;
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { audit_recorded, .. } => assert!(!audit_recorded),
        other => panic!("{other:?}"),
    }
    assert!(store.grants_for_subject("peer").unwrap().is_empty());
    *audit.fail.lock().unwrap() = false;
    access.retry_open_audit(&granted.resource_id).unwrap();
    let lines = audit.lines.lock().unwrap();
    let recorded = lines
        .iter()
        .find(|line| line.cause == "marker-missing")
        .unwrap();
    assert_eq!(recorded.initiator, "peer");
}

struct FailResource {
    inner: SqliteDirectoryStore,
    failed: Mutex<bool>,
}

impl DirectoryStore for FailResource {
    fn insert_resource(
        &self,
        resource: &StoredDirectoryResource,
        transition: &DirectoryAuditRecord,
    ) -> Result<(), DirectoryRefusal> {
        if !*self.failed.lock().unwrap() {
            *self.failed.lock().unwrap() = true;
            return Err(DirectoryRefusal::Store);
        }
        self.inner.insert_resource(resource, transition)
    }
    fn resource_at(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        self.inner.resource_at(environment, canonical)
    }
    fn resource(
        &self,
        resource_id: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        self.inner.resource(resource_id)
    }
    fn resource_by_binding(
        &self,
        binding: &[u8; BINDING_LEN],
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        self.inner.resource_by_binding(binding)
    }
    fn insert_grant(
        &self,
        grant: &StoredDirectoryGrant,
        initiator: &str,
    ) -> Result<(), DirectoryRefusal> {
        self.inner.insert_grant(grant, initiator)
    }
    fn grants_for_subject(
        &self,
        subject: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal> {
        self.inner.grants_for_subject(subject)
    }
    fn grants_for_resource(
        &self,
        resource_id: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal> {
        self.inner.grants_for_resource(resource_id)
    }
    fn revoke_mismatch(
        &self,
        resource_id: &str,
        cause: &str,
        initiator: &str,
    ) -> Result<RevokeMismatch, DirectoryRefusal> {
        self.inner.revoke_mismatch(resource_id, cause, initiator)
    }
    fn mark_open_audit(&self, resource_id: &str) -> Result<(), DirectoryRefusal> {
        self.inner.mark_open_audit(resource_id)
    }
    fn pending_open_audit(
        &self,
        resource_id: &str,
    ) -> Result<Option<PendingOpenAudit>, DirectoryRefusal> {
        self.inner.pending_open_audit(resource_id)
    }
    fn record_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
        initiator: &str,
    ) -> Result<(), DirectoryRefusal> {
        self.inner
            .record_cleanup(environment, canonical, binding, initiator)
    }
    fn cleanup(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<PendingCleanup>, DirectoryRefusal> {
        self.inner.cleanup(environment, canonical)
    }
    fn pending_cleanups(&self) -> Result<Vec<(String, String)>, DirectoryRefusal> {
        self.inner.pending_cleanups()
    }
    fn update_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        unlinked: bool,
        audit_done: bool,
    ) -> Result<(), DirectoryRefusal> {
        self.inner
            .update_cleanup(environment, canonical, unlinked, audit_done)
    }
    fn clear_cleanup(&self, environment: &str, canonical: &str) -> Result<(), DirectoryRefusal> {
        self.inner.clear_cleanup(environment, canonical)
    }
}

#[test]
fn a_failed_resource_insert_unlinks_the_marker_it_created() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let inner = SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(FailResource {
            inner,
            failed: Mutex::new(false),
        }),
        Arc::new(RecordingAudit::new()),
        [8; BINDING_LEN],
    );
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Store));
    assert!(!shared.join(".nessa-binding").exists());
}

struct LongPath;

impl DirectoryFiles for LongPath {
    fn canonicalize(&self, _: &str) -> Result<String, DirectoryRefusal> {
        Ok("x".repeat(MAX_ENCODED_GRANT_BYTES))
    }
    fn read_marker(&self, _: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        panic!("the frame check reads the marker");
    }
    fn create_marker(&self, _: &str, _: &[u8; BINDING_LEN]) -> Result<(), DirectoryRefusal> {
        panic!("created");
    }
    fn unlink_marker(&self, _: &str) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn walk(&self, _: &str, _: &str) -> Result<WalkFacts, DirectoryRefusal> {
        Ok(WalkFacts {
            leave: PathLeave::Inside,
            marker: false,
        })
    }
    fn read_inside(&self, _: &str, _: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        Ok(Vec::new())
    }
    fn write_inside(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn hides_marker(&self, _: SandboxKind) -> bool {
        true
    }
    fn root_leave(&self, _: &str, _: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        Ok(PathLeave::Inside)
    }
}

#[test]
fn a_grant_that_does_not_fit_one_frame_is_refused_before_the_marker_is_read() {
    let store_dir = private();
    let access = service(
        Arc::new(LongPath),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [1; BINDING_LEN],
    );
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: "/anywhere".into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(
        refused,
        DirectoryRefusal::Shape(crate::domain::DirectoryShapeError::Frame)
    ));
}

#[test]
fn a_write_only_grant_refuses_a_read_as_a_capability() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("note.txt"), b"hello").unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [2; BINDING_LEN],
    );
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["write"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap();
    let refused = access.read("peer", "note.txt").unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Capability { .. }));
}

#[test]
fn a_sandbox_root_beside_the_directory_is_outside() {
    let root = private();
    let shared = root.path().join("shared");
    let beside = root.path().join("sharedextra");
    fs::create_dir(&shared).unwrap();
    fs::create_dir(&beside).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [3; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    let refused = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Container,
            roots: &[beside.to_str().unwrap().to_owned()],
            hooks_answer_first: false,
        })
        .unwrap_err();
    match refused {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|id| id == "directory.exec_root"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn exec_of_a_recreated_directory_revokes_the_grant() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        Arc::new(RecordingAudit::new()),
        [4; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_dir_all(&shared).unwrap();
    fs::create_dir(&shared).unwrap();
    let refused = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Container,
            roots: &[shared.to_str().unwrap().to_owned()],
            hooks_answer_first: false,
        })
        .unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { cause, .. } => assert_eq!(cause, "marker-missing"),
        other => panic!("{other:?}"),
    }
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
}

#[test]
fn a_write_creates_a_file_that_was_not_there() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [5; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    access.write("peer", "fresh.txt", b"created").unwrap();
    assert_eq!(fs::read(shared.join("fresh.txt")).unwrap(), b"created");
}

#[test]
fn an_allowed_exec_is_not_reported_when_the_audit_sink_is_down() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(MarkerHidden {
            inner: LocalDirectoryFiles,
        }),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        audit.clone(),
        [6; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    *audit.fail.lock().unwrap() = true;
    let refused = access
        .exec(ExecAsk {
            subject: "peer",
            program: "ls",
            profile: SandboxKind::Container,
            roots: &[shared.to_str().unwrap().to_owned()],
            hooks_answer_first: false,
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Audit));
}

#[test]
fn a_share_is_not_reported_when_its_audit_cannot_be_recorded() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        audit.clone(),
        [7; BINDING_LEN],
    );
    *audit.fail.lock().unwrap() = true;
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Audit));
    assert!(store.grants_for_subject("peer").unwrap().is_empty());
    *audit.fail.lock().unwrap() = false;
    share_read(&access, shared.to_str().unwrap(), "peer");
    let lines = audit.lines.lock().unwrap();
    assert!(lines.iter().any(|line| line.cause == "first-share"));
}

struct OriginalSession;

impl DirectorySession for OriginalSession {
    fn marker(&self) -> Result<MarkerBytes, DirectoryRefusal> {
        Ok(MarkerBytes::Exact([9; BINDING_LEN]))
    }
    fn walk(&self, _: &str) -> Result<WalkFacts, DirectoryRefusal> {
        Ok(WalkFacts {
            leave: PathLeave::Inside,
            marker: false,
        })
    }
    fn read(&self, _: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        Ok(b"original".to_vec())
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn root_leave(&self, _: &str, _: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        Ok(PathLeave::Inside)
    }
}

struct MarkerHidden {
    inner: LocalDirectoryFiles,
}

impl DirectoryFiles for MarkerHidden {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        self.inner.canonicalize(path)
    }
    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        self.inner.read_marker(canonical)
    }
    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.create_marker(canonical, bytes)
    }
    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal> {
        self.inner.unlink_marker(canonical)
    }
    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        self.inner.walk(canonical, relative)
    }
    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        self.inner.read_inside(canonical, relative)
    }
    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.write_inside(canonical, relative, bytes)
    }
    fn hides_marker(&self, _: SandboxKind) -> bool {
        true
    }
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        self.inner.root_leave(canonical, roots)
    }
    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        self.inner.open_session(canonical)
    }
}

struct ReplacedPath;

impl DirectoryFiles for ReplacedPath {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        Ok(path.into())
    }
    fn read_marker(&self, _: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        Ok(MarkerBytes::Exact([9; BINDING_LEN]))
    }
    fn create_marker(&self, _: &str, _: &[u8; BINDING_LEN]) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn unlink_marker(&self, _: &str) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn walk(&self, _: &str, _: &str) -> Result<WalkFacts, DirectoryRefusal> {
        Ok(WalkFacts {
            leave: PathLeave::Inside,
            marker: false,
        })
    }
    fn read_inside(&self, _: &str, _: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        Ok(b"replaced".to_vec())
    }
    fn write_inside(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DirectoryRefusal> {
        Ok(())
    }
    fn hides_marker(&self, _: SandboxKind) -> bool {
        true
    }
    fn root_leave(&self, _: &str, _: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        Ok(PathLeave::Inside)
    }
    fn open_session(&self, _: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        Ok(Box::new(OriginalSession))
    }
}

#[test]
fn a_read_uses_the_directory_that_was_opened_not_a_later_path() {
    let store_dir = private();
    let access = service(
        Arc::new(ReplacedPath),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [9; BINDING_LEN],
    );
    share_read(&access, "/shared", "peer");
    assert_eq!(access.read("peer", "note.txt").unwrap(), b"original");
}

struct StickyUnlink {
    inner: LocalDirectoryFiles,
    fail: Mutex<bool>,
}

impl DirectoryFiles for StickyUnlink {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        self.inner.canonicalize(path)
    }
    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        self.inner.read_marker(canonical)
    }
    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.create_marker(canonical, bytes)
    }
    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal> {
        if *self.fail.lock().expect("unlink") {
            return Err(DirectoryRefusal::Io(std::io::ErrorKind::PermissionDenied));
        }
        self.inner.unlink_marker(canonical)
    }
    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        self.inner.walk(canonical, relative)
    }
    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        self.inner.read_inside(canonical, relative)
    }
    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.write_inside(canonical, relative, bytes)
    }
    fn hides_marker(&self, profile: SandboxKind) -> bool {
        self.inner.hides_marker(profile)
    }
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        self.inner.root_leave(canonical, roots)
    }
    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        self.inner.open_session(canonical)
    }
}

#[test]
fn a_failed_unlink_does_not_record_the_marker_as_absent() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let files = Arc::new(StickyUnlink {
        inner: LocalDirectoryFiles,
        fail: Mutex::new(true),
    });
    let inner = SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap();
    let access = service(
        files.clone(),
        Arc::new(FailResource {
            inner,
            failed: Mutex::new(false),
        }),
        audit.clone(),
        [8; BINDING_LEN],
    );
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Store));
    assert!(shared.join(".nessa-binding").is_file());
    assert!(audit
        .lines
        .lock()
        .unwrap()
        .iter()
        .all(|line| line.cause != "file-absent"));
    let still = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(still, DirectoryRefusal::CleanupPending));
    assert!(shared.join(".nessa-binding").is_file());
    *files.fail.lock().unwrap() = false;
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert!(audit
        .lines
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.cause == "file-absent"));
}

#[test]
fn a_shell_string_is_a_constraint_only_when_the_grant_has_command_rules() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [1; BINDING_LEN],
    );
    assert!(!access.command_constrained("owner").unwrap());
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert!(!access.command_constrained("peer").unwrap());
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("ruled".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["exec"]).unwrap(),
            rules: crate::domain::CommandRules::new(vec![allow_program("ls").unwrap()]).unwrap(),
            initiator: "owner".into(),
        })
        .unwrap();
    assert!(access.command_constrained("ruled").unwrap());
}

#[test]
fn a_deny_on_one_directory_refuses_even_when_another_grant_has_no_rules() {
    let root = private();
    let open = root.path().join("open");
    let closed = root.path().join("closed");
    fs::create_dir(&open).unwrap();
    fs::create_dir(&closed).unwrap();
    let store_dir = private();
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        audit.clone(),
        [2; BINDING_LEN],
    );
    let closed_access = service(
        Arc::new(LocalDirectoryFiles),
        store,
        audit,
        [3; BINDING_LEN],
    );
    share_read(&access, open.to_str().unwrap(), "peer");
    closed_access
        .share(ShareDirectory {
            environment: "local".into(),
            path: closed.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["exec"]).unwrap(),
            rules: crate::domain::CommandRules::new(vec![deny_argv("rm", "-*").unwrap()]).unwrap(),
            initiator: "owner".into(),
        })
        .unwrap();
    let refused = access
        .command_at("peer", "rm", &["-rf".into()])
        .unwrap_err();
    match refused {
        DirectoryRefusal::Program { matcher, .. } => assert_eq!(matcher, "command.rules.deny.0"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failed_second_share_does_not_keep_a_grant_change() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let path = store_dir.path().join("grants.sqlite3");
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&path).unwrap()),
        Arc::new(RecordingAudit::new()),
        [3; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    let again = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(again, DirectoryRefusal::Store));
    let connection = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    let changes: i64 = connection
        .query_row(
            "SELECT count(*) FROM share_grant_changes WHERE resource_kind = 'directory'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let grants: i64 = connection
        .query_row(
            "SELECT count(*) FROM share_grants WHERE resource_kind = 'directory'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(changes, 1);
    assert_eq!(grants, 1);
}

#[test]
fn deleting_the_directory_revokes_the_grant() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        Arc::new(RecordingAudit::new()),
        [4; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_dir_all(&shared).unwrap();
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { cause, .. } => assert_eq!(cause, "marker-missing"),
        other => panic!("{other:?}"),
    }
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
}

#[test]
fn a_share_after_revocation_is_revoked_again_on_the_next_mismatch() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        Arc::new(RecordingAudit::new()),
        [8; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    let marker = fs::read(shared.join(".nessa-binding")).unwrap();
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    access.read("peer", "note.txt").unwrap_err();
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
    fs::write(shared.join(".nessa-binding"), &marker).unwrap();
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert_eq!(
        store
            .grants_for_resource(&granted.resource_id)
            .unwrap()
            .len(),
        1
    );
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { cause, .. } => assert_eq!(cause, "marker-missing"),
        other => panic!("{other:?}"),
    }
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
    fs::write(shared.join(".nessa-binding"), &marker).unwrap();
    assert!(matches!(
        access.read("peer", "note.txt").unwrap_err(),
        DirectoryRefusal::NotFound
    ));
}

#[test]
fn a_partial_revocation_journal_does_not_stick() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let store = Arc::new(SqliteDirectoryStore::open(&database).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        Arc::new(RecordingAudit::new()),
        [9; BINDING_LEN],
    );
    let granted = share_read(&access, shared.to_str().unwrap(), "peer");
    share_read(&access, shared.to_str().unwrap(), "other");
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    let probe = nessa_local_database::rusqlite::Connection::open(&database).unwrap();
    probe
        .execute_batch(
            "CREATE TRIGGER directory_revoke_fail_second
             BEFORE INSERT ON share_grant_changes
             WHEN NEW.after = 'none'
               AND (SELECT COUNT(*) FROM share_grant_changes
                    WHERE resource_kind = 'directory' AND resource_id = NEW.resource_id AND after = 'none') >= 1
             BEGIN
               SELECT RAISE(ABORT, 'forced second revoke');
             END;",
        )
        .unwrap();
    assert!(matches!(
        access.read("peer", "note.txt").unwrap_err(),
        DirectoryRefusal::Store
    ));
    assert_eq!(
        store
            .grants_for_resource(&granted.resource_id)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        count(&probe, "SELECT count(*) FROM directory_open_audits"),
        0
    );
    assert_eq!(
        count(
            &probe,
            "SELECT count(*) FROM share_grant_changes WHERE after = 'none'"
        ),
        0
    );
    probe
        .execute_batch("DROP TRIGGER directory_revoke_fail_second;")
        .unwrap();
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { cause, .. } => assert_eq!(cause, "marker-missing"),
        other => panic!("{other:?}"),
    }
    assert!(store
        .grants_for_resource(&granted.resource_id)
        .unwrap()
        .is_empty());
}

fn count(connection: &nessa_local_database::rusqlite::Connection, sql: &str) -> i64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn a_later_read_delivers_the_mismatch_audit_the_sink_refused() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store,
        audit.clone(),
        [11; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    *audit.fail.lock().unwrap() = true;
    let refused = access.read("peer", "note.txt").unwrap_err();
    match refused {
        DirectoryRefusal::MarkerMismatch { audit_recorded, .. } => assert!(!audit_recorded),
        other => panic!("{other:?}"),
    }
    *audit.fail.lock().unwrap() = false;
    assert!(matches!(
        access.read("peer", "note.txt").unwrap_err(),
        DirectoryRefusal::NotFound
    ));
    let lines = audit.lines.lock().unwrap();
    let recorded = lines
        .iter()
        .find(|line| line.cause == "marker-missing")
        .unwrap();
    assert_eq!(recorded.initiator, "peer");
    assert_eq!(recorded.before.as_deref(), Some("active"));
    assert_eq!(recorded.after.as_deref(), Some("absent"));
}

#[test]
fn restart_delivers_a_mismatch_audit_without_another_open() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let failing = Arc::new(RecordingAudit::new());
    let store = Arc::new(SqliteDirectoryStore::open(&database).unwrap());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        store,
        failing.clone(),
        [12; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    fs::remove_file(shared.join(".nessa-binding")).unwrap();
    *failing.fail.lock().unwrap() = true;
    access.read("peer", "note.txt").unwrap_err();
    drop(access);
    let restarted_audit = Arc::new(RecordingAudit::new());
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        restarted_audit.clone(),
        [12; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    let lines = restarted_audit.lines.lock().unwrap();
    let recorded = lines
        .iter()
        .find(|line| line.cause == "marker-missing")
        .unwrap();
    assert_eq!(recorded.initiator, "peer");
}

#[test]
fn a_symlink_and_its_target_are_one_directory() {
    let root = private();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [13; BINDING_LEN],
    );
    let through_alias = share_read(&access, alias.to_str().unwrap(), "peer");
    let through_real = share_read(&access, real.to_str().unwrap(), "other");
    let expected = std::fs::canonicalize(&real).unwrap();
    assert_eq!(through_alias.canonical_path, expected.to_str().unwrap());
    assert_eq!(through_alias.resource_id, through_real.resource_id);
    assert_eq!(through_alias.canonical_path, through_real.canonical_path);
}

#[test]
fn an_unasked_call_is_not_forgotten_when_the_audit_sink_is_down() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        audit.clone(),
        [5; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    *audit.fail.lock().unwrap() = true;
    let refused = access.report_unasked("peer", "Bash").unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Audit));
    assert!(audit
        .lines
        .lock()
        .unwrap()
        .iter()
        .all(|line| line.cause != "outside-nessa-gate"));
    *audit.fail.lock().unwrap() = false;
    access.report_unasked("peer", "Bash").unwrap();
    assert!(audit
        .lines
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.cause == "outside-nessa-gate"));
}

struct FailUnlink {
    inner: LocalDirectoryFiles,
    fail: Mutex<bool>,
}

impl DirectoryFiles for FailUnlink {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        self.inner.canonicalize(path)
    }
    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        self.inner.read_marker(canonical)
    }
    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.create_marker(canonical, bytes)
    }
    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal> {
        if *self.fail.lock().unwrap() {
            *self.fail.lock().unwrap() = false;
            return Err(DirectoryRefusal::Io(std::io::ErrorKind::PermissionDenied));
        }
        self.inner.unlink_marker(canonical)
    }
    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        self.inner.walk(canonical, relative)
    }
    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        self.inner.read_inside(canonical, relative)
    }
    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.write_inside(canonical, relative, bytes)
    }
    fn hides_marker(&self, profile: crate::domain::SandboxKind) -> bool {
        self.inner.hides_marker(profile)
    }
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        self.inner.root_leave(canonical, roots)
    }
    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        self.inner.open_session(canonical)
    }
}

#[test]
fn a_failed_cleanup_is_finished_without_sharing_that_path_again() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let files = Arc::new(FailUnlink {
        inner: LocalDirectoryFiles,
        fail: Mutex::new(true),
    });
    let store = Arc::new(FailResource {
        inner: SqliteDirectoryStore::open(&database).unwrap(),
        failed: Mutex::new(false),
    });
    let access = service(
        files,
        store,
        Arc::new(RecordingAudit::new()),
        [14; BINDING_LEN],
    );
    let refused = access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::Store));
    assert!(shared.join(".nessa-binding").is_file());
    let audit = Arc::new(RecordingAudit::new());
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        audit.clone(),
        [14; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    assert!(!shared.join(".nessa-binding").exists());
    let lines = audit.lines.lock().unwrap();
    let recorded = lines
        .iter()
        .find(|line| line.cause == "file-absent")
        .unwrap();
    assert_eq!(recorded.initiator, "owner");
    assert_eq!(recorded.after.as_deref(), Some("absent"));
}

#[test]
fn an_unenclosed_command_is_not_admitted_for_a_directory_grant() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [15; BINDING_LEN],
    );
    access
        .admit_execution(
            "stranger",
            "cat",
            &[".nessa-binding".into()],
            SandboxKind::None,
            &[],
        )
        .unwrap();
    share_read(&access, shared.to_str().unwrap(), "peer");
    let refused = access
        .admit_execution(
            "peer",
            "cat",
            &[".nessa-binding".into()],
            SandboxKind::None,
            &[],
        )
        .unwrap_err();
    match refused {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|rule| rule == "directory.exec_none"));
        }
        other => panic!("{other:?}"),
    }
    access
        .admit_shell("stranger", SandboxKind::None, &[])
        .unwrap();
    let quoted = access
        .admit_shell("peer", SandboxKind::None, &[])
        .unwrap_err();
    match quoted {
        DirectoryRefusal::Exec { rules, .. } => {
            assert!(rules.iter().any(|rule| rule == "directory.exec_none"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_failed_audit_retry_still_revokes_another_directory() {
    let root = private();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let third = root.path().join("third");
    let fourth = root.path().join("fourth");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::create_dir(&third).unwrap();
    fs::create_dir(&fourth).unwrap();
    fs::write(third.join("note.txt"), b"kept").unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let store =
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap());
    let access = DirectoryAccess::new(
        Arc::new(LocalDirectoryFiles),
        store.clone(),
        Arc::new(CedarDirectoryPolicyEvaluator::new().unwrap()),
        audit.clone(),
        Arc::new(RandomBinding),
    );
    let revoked = share_read(&access, first.to_str().unwrap(), "alfa");
    let other = share_read(&access, second.to_str().unwrap(), "beta");
    share_read(&access, third.to_str().unwrap(), "calm");
    let executed = share_read(&access, fourth.to_str().unwrap(), "delta");
    fs::remove_file(first.join(".nessa-binding")).unwrap();
    *audit.fail.lock().unwrap() = true;
    access.read("alfa", "note.txt").unwrap_err();
    assert!(store
        .grants_for_resource(&revoked.resource_id)
        .unwrap()
        .is_empty());
    fs::remove_file(second.join(".nessa-binding")).unwrap();
    let refused = access.read("beta", "note.txt").unwrap_err();
    assert!(matches!(refused, DirectoryRefusal::MarkerMismatch { .. }));
    assert!(store
        .grants_for_resource(&other.resource_id)
        .unwrap()
        .is_empty());
    assert!(matches!(
        access.read("calm", "note.txt").unwrap_err(),
        DirectoryRefusal::Store
    ));
    fs::remove_file(fourth.join(".nessa-binding")).unwrap();
    assert!(matches!(
        access
            .exec(ExecAsk {
                subject: "delta",
                program: "cat",
                profile: SandboxKind::None,
                roots: &[],
                hooks_answer_first: false,
            })
            .unwrap_err(),
        DirectoryRefusal::MarkerMismatch { .. }
    ));
    assert!(store
        .grants_for_resource(&executed.resource_id)
        .unwrap()
        .is_empty());
}

struct GateUnlink {
    inner: LocalDirectoryFiles,
    fail_once: Mutex<bool>,
    entered: std::sync::Condvar,
    entered_flag: Mutex<bool>,
    release: Mutex<bool>,
    release_cv: std::sync::Condvar,
}

impl DirectoryFiles for GateUnlink {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        self.inner.canonicalize(path)
    }
    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        self.inner.read_marker(canonical)
    }
    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.create_marker(canonical, bytes)
    }
    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal> {
        if *self.fail_once.lock().unwrap() {
            *self.fail_once.lock().unwrap() = false;
            return Err(DirectoryRefusal::Io(std::io::ErrorKind::PermissionDenied));
        }
        {
            let mut entered = self.entered_flag.lock().unwrap();
            *entered = true;
            self.entered.notify_all();
        }
        let mut release = self.release.lock().unwrap();
        while !*release {
            release = self.release_cv.wait(release).unwrap();
        }
        self.inner.unlink_marker(canonical)
    }
    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        self.inner.walk(canonical, relative)
    }
    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        self.inner.read_inside(canonical, relative)
    }
    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        self.inner.write_inside(canonical, relative, bytes)
    }
    fn hides_marker(&self, profile: SandboxKind) -> bool {
        self.inner.hides_marker(profile)
    }
    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        self.inner.root_leave(canonical, roots)
    }
    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        self.inner.open_session(canonical)
    }
}

#[test]
fn cleanup_waits_for_the_share_lease_before_it_unlinks() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let files = Arc::new(GateUnlink {
        inner: LocalDirectoryFiles,
        fail_once: Mutex::new(true),
        entered: std::sync::Condvar::new(),
        entered_flag: Mutex::new(false),
        release: Mutex::new(false),
        release_cv: std::sync::Condvar::new(),
    });
    let access = Arc::new(service(
        files.clone(),
        Arc::new(FailResource {
            inner: SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap(),
            failed: Mutex::new(false),
        }),
        Arc::new(RecordingAudit::new()),
        [17; BINDING_LEN],
    ));
    let request = ShareDirectory {
        environment: "local".into(),
        path: shared.to_str().unwrap().into(),
        subject: DirectorySubject {
            principal_id: Some("peer".into()),
            receiver_id: None,
        },
        capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
        rules: crate::domain::CommandRules::none(),
        initiator: "owner".into(),
    };
    assert!(matches!(
        access.share(request.clone()).unwrap_err(),
        DirectoryRefusal::Store
    ));
    assert!(shared.join(".nessa-binding").is_file());
    let reconciling = access.clone();
    let reconcile_thread = std::thread::spawn(move || reconciling.reconcile());
    {
        let mut entered = files.entered_flag.lock().unwrap();
        while !*entered {
            entered = files.entered.wait(entered).unwrap();
        }
    }
    let sharing = access.clone();
    let shared_again = request.clone();
    let share_thread = std::thread::spawn(move || sharing.share(shared_again));
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_millis(200) && !share_thread.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    *files.release.lock().unwrap() = true;
    files.release_cv.notify_all();
    reconcile_thread.join().unwrap().unwrap();
    share_thread.join().unwrap().unwrap();
    assert!(shared.join(".nessa-binding").is_file());
}

#[test]
fn a_refused_read_or_write_records_who_asked_and_the_path() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("note.txt"), b"hello").unwrap();
    let store_dir = private();
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        audit.clone(),
        [19; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    assert!(matches!(
        access.read("peer", ".nessa-binding").unwrap_err(),
        DirectoryRefusal::MarkerName
    ));
    assert!(matches!(
        access.write("peer", "../outside.txt", b"no").unwrap_err(),
        DirectoryRefusal::Outside { .. }
    ));
    let lines = audit.lines.lock().unwrap();
    let marker = lines
        .iter()
        .find(|line| line.cause == "marker-name")
        .unwrap();
    assert_eq!(marker.initiator, "peer");
    assert_eq!(marker.before.as_deref(), Some("directory.read"));
    assert_eq!(marker.after.as_deref(), Some(".nessa-binding"));
    let outside = lines
        .iter()
        .find(|line| line.cause == "path-leaves")
        .unwrap();
    assert_eq!(outside.initiator, "peer");
    assert_eq!(outside.before.as_deref(), Some("directory.write"));
    assert_eq!(outside.after.as_deref(), Some("../outside.txt"));
}

#[test]
fn a_share_keeps_its_audit_when_the_sink_is_down_until_restart() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let failing = Arc::new(RecordingAudit::new());
    *failing.fail.lock().unwrap() = true;
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        failing,
        [18; BINDING_LEN],
    );
    assert!(matches!(
        access
            .share(ShareDirectory {
                environment: "local".into(),
                path: shared.to_str().unwrap().into(),
                subject: DirectorySubject {
                    principal_id: Some("peer".into()),
                    receiver_id: None,
                },
                capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
                rules: crate::domain::CommandRules::none(),
                initiator: "owner".into(),
            })
            .unwrap_err(),
        DirectoryRefusal::Audit
    ));
    drop(access);
    let audit = Arc::new(RecordingAudit::new());
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        audit.clone(),
        [18; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    let lines = audit.lines.lock().unwrap();
    let recorded = lines
        .iter()
        .find(|line| line.cause == "first-share")
        .unwrap();
    assert_eq!(recorded.initiator, "owner");
    assert_eq!(recorded.before.as_deref(), Some("none"));
    assert_eq!(recorded.after.as_deref(), Some("active"));
}

#[test]
fn a_refused_exec_is_kept_when_the_audit_sink_is_down() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let audit = Arc::new(RecordingAudit::new());
    let access = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        audit.clone(),
        [20; BINDING_LEN],
    );
    share_read(&access, shared.to_str().unwrap(), "peer");
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("ruled".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["exec"]).unwrap(),
            rules: crate::domain::CommandRules::new(vec![allow_program("ls").unwrap()]).unwrap(),
            initiator: "owner".into(),
        })
        .unwrap();
    *audit.fail.lock().unwrap() = true;
    let refused = access
        .exec(ExecAsk {
            subject: "peer",
            program: "cat",
            profile: SandboxKind::None,
            roots: &[],
            hooks_answer_first: false,
        })
        .unwrap_err();
    match refused {
        DirectoryRefusal::Exec { audit_recorded, .. } => assert!(!audit_recorded),
        other => panic!("{other:?}"),
    }
    *audit.fail.lock().unwrap() = false;
    access.reconcile().unwrap();
    *audit.fail.lock().unwrap() = true;
    let command = access.command_at("ruled", "rm", &[]).unwrap_err();
    match command {
        DirectoryRefusal::Program { audit_recorded, .. } => assert!(!audit_recorded),
        other => panic!("{other:?}"),
    }
    *audit.fail.lock().unwrap() = false;
    access.reconcile().unwrap();
    let lines = audit.lines.lock().unwrap();
    assert!(lines
        .iter()
        .any(|line| line.cause == "exec-refused" && line.initiator == "peer"));
    let command = lines
        .iter()
        .find(|line| line.cause == "command-refused" && line.initiator == "ruled")
        .unwrap();
    assert_eq!(command.after.as_deref(), Some("rm"));
    assert!(command
        .before
        .as_deref()
        .is_some_and(|matcher| matcher.starts_with("command.rules")));
    assert!(command
        .profile
        .as_deref()
        .is_some_and(|rules| rules.contains("command.rules")));
}

#[test]
fn cleanup_does_not_unlink_a_marker_this_attempt_did_not_create() {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let files = Arc::new(FailUnlink {
        inner: LocalDirectoryFiles,
        fail: Mutex::new(true),
    });
    let access = service(
        files,
        Arc::new(FailResource {
            inner: SqliteDirectoryStore::open(&database).unwrap(),
            failed: Mutex::new(false),
        }),
        Arc::new(RecordingAudit::new()),
        [21; BINDING_LEN],
    );
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    fs::write(shared.join(".nessa-binding"), [9; BINDING_LEN]).unwrap();
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&database).unwrap()),
        Arc::new(RecordingAudit::new()),
        [21; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    assert_eq!(
        fs::read(shared.join(".nessa-binding")).unwrap(),
        [9; BINDING_LEN]
    );
}

fn pending_cleanup(binding: u8) -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let root = private();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let store_dir = private();
    let database = store_dir.path().join("grants.sqlite3");
    let access = service(
        Arc::new(FailUnlink {
            inner: LocalDirectoryFiles,
            fail: Mutex::new(true),
        }),
        Arc::new(FailResource {
            inner: SqliteDirectoryStore::open(&database).unwrap(),
            failed: Mutex::new(false),
        }),
        Arc::new(RecordingAudit::new()),
        [binding; BINDING_LEN],
    );
    access
        .share(ShareDirectory {
            environment: "local".into(),
            path: shared.to_str().unwrap().into(),
            subject: DirectorySubject {
                principal_id: Some("peer".into()),
                receiver_id: None,
            },
            capabilities: crate::domain::DirectoryCapabilities::from_names(["read"]).unwrap(),
            rules: crate::domain::CommandRules::none(),
            initiator: "owner".into(),
        })
        .unwrap_err();
    (root, store_dir, shared)
}

#[test]
fn cleanup_finishes_when_the_directory_is_already_gone() {
    let (_root, store_dir, shared) = pending_cleanup(22);
    fs::remove_dir_all(&shared).unwrap();
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [22; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    restarted.reconcile().unwrap();
}

#[test]
fn cleanup_leaves_a_marker_in_a_directory_that_replaced_the_original() {
    let (_root, store_dir, shared) = pending_cleanup(23);
    fs::remove_dir_all(&shared).unwrap();
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join(".nessa-binding"), [9; BINDING_LEN]).unwrap();
    let restarted = service(
        Arc::new(LocalDirectoryFiles),
        Arc::new(SqliteDirectoryStore::open(&store_dir.path().join("grants.sqlite3")).unwrap()),
        Arc::new(RecordingAudit::new()),
        [23; BINDING_LEN],
    );
    restarted.reconcile().unwrap();
    assert_eq!(
        fs::read(shared.join(".nessa-binding")).unwrap(),
        [9; BINDING_LEN]
    );
}
