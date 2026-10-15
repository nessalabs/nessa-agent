//! The live remembered permission rules, and the reviews waiting on a person.
//!
//! `settle` asks Cedar as the caller the host passes. A miss, or a policy
//! that cannot be asked, asks the person. `remember` and `revoke` write the
//! rule and its audit row in one database transaction, under the same lock
//! as the cache, so a revoke cannot land between the commit and the rows a
//! settle reads. Cedar admits or refuses the caller; the owner is not
//! assumed. A failure leaves the file as it was.
//!
//! A share that changes who may read a conversation asks the waiting-review
//! publishers to filter again, without moving the board's generation.

use nessa_auth::application::ports::Clock;
use nessa_local_database::OpenError;
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, PermissionCaller, PermissionRules, RememberError,
    RememberRequest, ReviewNotice, ReviewNotices, SettledRule,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::agent_execution::permissions::{PermissionEffect, PermissionId};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::watch;

use crate::conversation::application::{
    rule_principal, ListedPermissionRule, PermissionRuleCatalog, PermissionRulePolicyEvaluator,
    RuleCatalogError, RuleChange, RulePrincipal, RuleSettlement, StoredPermissionRule,
};

use super::cedar::CedarPermissionRuleEvaluator;
use super::store::{LoadError, RememberWrite, RuleStore, WriteError};

const MAX_ARMS: usize = 1024;
const MAX_NOTICES: usize = 128;
/// How many waiting reviews one notice carries. The board keeps the same
/// number, so a replacement snapshot is every review still waiting. One
/// execution admits at most 128 pending permissions, and the wire schema
/// states the same bound.
const PUBLISHED_REVIEWS: usize = MAX_NOTICES;
/// How many rules `conversation.permissionRules` may return. The wire schema
/// states the same bound. A new key past it is not stored; replacing a key is.
const MAX_RULES: usize = 256;

/// One waiting review, as a notice names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedReview {
    /// Conversation the review belongs to.
    pub conversation_id: String,
    /// Execution that owns the review.
    pub execution_id: String,
    /// Pending permission identity.
    pub permission_id: String,
    /// Tool the review named.
    pub tool: String,
}

/// Why the rules file was not opened.
#[derive(Debug)]
pub enum RulesOpenError {
    /// The SQLite file was not opened.
    Store(OpenError),
    /// The Cedar schema or policies did not validate.
    Policy,
}

struct Arms {
    order: VecDeque<String>,
    owner: HashMap<String, bool>,
}

struct NotedCallers {
    order: VecDeque<String>,
    callers: HashMap<String, PermissionCaller>,
}

/// The file and the rows a settle reads. One lock covers both, so a commit
/// and the cache update cannot be split by a revoke.
struct LiveRules {
    store: RuleStore,
    rows: Vec<StoredPermissionRule>,
}

/// Contents and their authoritative versions commit together. Watch channels
/// project wakeups only; a delayed notification is not an admission token.
struct WaitingReviews {
    notices: Vec<PublishedReview>,
    generation: u64,
    visibility: u64,
}

/// Rules, who may be offered a remember choice, and the reviews still waiting.
pub struct PermissionMemory {
    rules: Mutex<LiveRules>,
    arms: Mutex<Arms>,
    noted: Mutex<NotedCallers>,
    waiting: Mutex<WaitingReviews>,
    generation: watch::Sender<u64>,
    /// Moves when who may see a waiting review may have changed.
    visibility: watch::Sender<u64>,
    evaluator: CedarPermissionRuleEvaluator,
    clock: Arc<dyn Clock>,
}

impl PermissionMemory {
    /// Open the rules file, skipping rows that are not rules.
    ///
    /// `clock` dates an audit row. It is the time this process observed the
    /// commit, not a time the caller claimed. The Cedar bundle is validated
    /// here, so a broken policy refuses to open.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, RulesOpenError> {
        let evaluator =
            CedarPermissionRuleEvaluator::validated().map_err(|_| RulesOpenError::Policy)?;
        let store = RuleStore::open(path).map_err(RulesOpenError::Store)?;
        let rows = store.load().map_err(|error| match error {
            LoadError::Database(cause) => RulesOpenError::Store(OpenError::Database(cause)),
            LoadError::NotARule => RulesOpenError::Store(OpenError::Damaged(
                "a permission rule row is not a rule".to_owned(),
            )),
        })?;
        let (generation, _) = watch::channel(0);
        let (visibility, _) = watch::channel(0);
        Ok(Self {
            rules: Mutex::new(LiveRules { store, rows }),
            arms: Mutex::new(Arms {
                order: VecDeque::new(),
                owner: HashMap::new(),
            }),
            noted: Mutex::new(NotedCallers {
                order: VecDeque::new(),
                callers: HashMap::new(),
            }),
            waiting: Mutex::new(WaitingReviews {
                notices: Vec::new(),
                generation: 0,
                visibility: 0,
            }),
            generation,
            visibility,
            evaluator,
            clock,
        })
    }

    /// Whether Cedar permits `principal` to make `change`.
    ///
    /// [`RuleCatalogError::Refused`] when Cedar denies it.
    /// [`RuleCatalogError::Unavailable`] when Cedar cannot be asked. A change
    /// that cannot be authorized is not made.
    pub fn authorize(
        &self,
        principal: RulePrincipal,
        change: RuleChange,
    ) -> Result<(), RuleCatalogError> {
        match self.evaluator.may(principal, change) {
            Ok(true) => Ok(()),
            Ok(false) => Err(RuleCatalogError::Refused),
            Err(_) => Err(RuleCatalogError::Unavailable),
        }
    }

    /// The generation and the waiting reviews a notice should carry.
    pub fn current(&self) -> (u64, Vec<PublishedReview>) {
        let waiting = lock(&self.waiting);
        let skip = waiting.notices.len().saturating_sub(PUBLISHED_REVIEWS);
        (
            waiting.generation,
            waiting.notices.iter().skip(skip).cloned().collect(),
        )
    }

    /// Whether both shared versions still match a filtered candidate, without
    /// cloning its notices. Audited publishers check before snapshot admission
    /// (`review_notice_reselects_after_shared_changes_during_audit`).
    pub fn is_current(&self, generation: u64, visibility: u64) -> bool {
        let waiting = lock(&self.waiting);
        generation != u64::MAX
            && visibility != u64::MAX
            && waiting.generation == generation
            && waiting.visibility == visibility
    }

    /// Wait until the generation is no longer `seen`.
    pub async fn wait_after(&self, seen: u64) {
        let mut updates = self.generation.subscribe();
        loop {
            drop(updates.borrow_and_update());
            if lock(&self.waiting).generation != seen {
                return;
            }
            if updates.changed().await.is_err() {
                return;
            }
        }
    }

    /// The visibility generation a notice publisher last filtered at.
    pub fn visibility(&self) -> u64 {
        lock(&self.waiting).visibility
    }

    /// Ask every waiting-review publisher to filter its snapshot again.
    ///
    /// A share or an unshare does not change the board's generation. A session
    /// that could see a review can lose that conversation while the review is
    /// still waiting, and the next snapshot is the filtered set.
    pub fn recheck_visibility(&self) {
        let next = {
            let mut waiting = lock(&self.waiting);
            waiting.visibility = waiting.visibility.saturating_add(1);
            waiting.visibility
        };
        publish_version(&self.visibility, next);
    }

    /// Wait until visibility should be filtered again.
    pub async fn wait_for_visibility(&self, seen: u64) {
        let mut updates = self.visibility.subscribe();
        loop {
            drop(updates.borrow_and_update());
            if lock(&self.waiting).visibility != seen {
                return;
            }
            if updates.changed().await.is_err() {
                return;
            }
        }
    }
}

fn publish_version(projection: &watch::Sender<u64>, version: u64) {
    projection.send_if_modified(|published| {
        if *published < version {
            *published = version;
            true
        } else {
            false
        }
    });
}

impl PermissionRules for PermissionMemory {
    fn settle(
        &self,
        caller: PermissionCaller,
        workspace: &str,
        source: &str,
        tool: &str,
    ) -> Option<SettledRule> {
        if workspace.is_empty() || tool.is_empty() || (source != "claude" && source != "codex") {
            return None;
        }
        let rules = lock(&self.rules).rows.clone();
        let settlement = self
            .evaluator
            .settle(rule_principal(caller), workspace, source, tool, &rules)
            .ok()?;
        let (rule_id, effect) = match settlement {
            RuleSettlement::Ask => return None,
            RuleSettlement::Allow { rule_id } => (rule_id, PermissionEffect::Allow),
            RuleSettlement::Deny { rule_id } => (rule_id, PermissionEffect::Deny),
        };
        let rule = rules
            .iter()
            .find(|rule| rule.id == rule_id && rule.effect == effect)?;
        let reference =
            ApprovalRuleReference::new(&rule.id, &rule.revision, rule.granted_by.clone()).ok()?;
        Some(SettledRule::new(reference, effect))
    }

    fn offers_remember(&self, conversation: &str, execution: &ExecutionId) -> bool {
        lock(&self.arms)
            .owner
            .get(&scope_key(conversation, execution))
            .copied()
            .unwrap_or(false)
    }

    fn arm(&self, conversation: &str, execution: &ExecutionId, owner: bool) {
        let mut arms = lock(&self.arms);
        let id = scope_key(conversation, execution);
        if arms.owner.insert(id.clone(), owner).is_none() {
            arms.order.push_back(id);
            while arms.order.len() > MAX_ARMS {
                if let Some(old) = arms.order.pop_front() {
                    arms.owner.remove(&old);
                }
            }
        }
    }

    fn note_answerer(
        &self,
        conversation: &str,
        execution: &ExecutionId,
        request_id: &str,
        caller: PermissionCaller,
    ) {
        let mut noted = lock(&self.noted);
        let key = noted_key(conversation, execution, request_id);
        if noted.callers.insert(key.clone(), caller).is_none() {
            noted.order.push_back(key);
            while noted.order.len() > MAX_ARMS {
                if let Some(old) = noted.order.pop_front() {
                    noted.callers.remove(&old);
                }
            }
        }
    }

    fn answerer(
        &self,
        conversation: &str,
        execution: &ExecutionId,
        request_id: &str,
    ) -> Option<PermissionCaller> {
        lock(&self.noted)
            .callers
            .remove(&noted_key(conversation, execution, request_id))
    }

    fn remember(&self, request: RememberRequest<'_>) -> Result<SettledRule, RememberError> {
        if !self.offers_remember(request.conversation_id, request.execution_id)
            || request.workspace.is_empty()
            || request.tool.is_empty()
            || (request.source != "claude" && request.source != "codex")
        {
            return Err(RememberError::Refused);
        }
        match self
            .evaluator
            .may(rule_principal(request.caller), RuleChange::Grant)
        {
            Ok(true) => {}
            Ok(false) => return Err(RememberError::Refused),
            Err(_) => return Err(RememberError::Unavailable),
        }
        let mut rules = lock(&self.rules);
        let replacing = rules.rows.iter().any(|rule| {
            rule.workspace == request.workspace
                && rule.source == request.source
                && rule.tool == request.tool
        });
        if !replacing && rules.rows.len() >= MAX_RULES {
            return Err(RememberError::Unavailable);
        }
        let id = format!("pr_{}", uuid::Uuid::new_v4().simple());
        let observed = self.clock.unix_milliseconds();
        let stored = rules
            .store
            .remember(RememberWrite {
                workspace: request.workspace,
                source: request.source,
                tool: request.tool,
                effect: request.effect,
                granted_by: request.granted_by,
                fresh_id: id,
                observed_at_ms: observed,
            })
            .map_err(|error| match error {
                WriteError::Database | WriteError::Revision => RememberError::Unavailable,
            })?;
        replace_row(&mut rules.rows, stored.clone());
        let reference = ApprovalRuleReference::new(&stored.id, &stored.revision, stored.granted_by)
            .map_err(|_| RememberError::Unavailable)?;
        Ok(SettledRule::new(reference, stored.effect))
    }
}

impl ReviewNotices for PermissionMemory {
    fn waiting(&self, notice: &ReviewNotice) {
        let execution_id = notice.execution_id.as_str().to_owned();
        let permission_id = notice.permission_id.as_str().to_owned();
        let mut waiting = lock(&self.waiting);
        if let Some(existing) = waiting.notices.iter_mut().find(|item| {
            item.conversation_id == notice.conversation_id
                && item.execution_id == execution_id
                && item.permission_id == permission_id
        }) {
            if existing.tool == notice.tool {
                return;
            }
            existing.tool.clone_from(&notice.tool);
        } else {
            waiting.notices.push(PublishedReview {
                conversation_id: notice.conversation_id.clone(),
                execution_id,
                permission_id,
                tool: notice.tool.clone(),
            });
            while waiting.notices.len() > MAX_NOTICES {
                waiting.notices.remove(0);
            }
        }
        waiting.generation = waiting.generation.saturating_add(1);
        let generation = waiting.generation;
        drop(waiting);
        publish_version(&self.generation, generation);
    }

    fn clear(&self, conversation: &str, execution: &ExecutionId, permission: &PermissionId) {
        let mut waiting = lock(&self.waiting);
        let before = waiting.notices.len();
        waiting.notices.retain(|item| {
            item.conversation_id != conversation
                || item.execution_id != execution.as_str()
                || item.permission_id != permission.as_str()
        });
        if waiting.notices.len() != before {
            waiting.generation = waiting.generation.saturating_add(1);
            let generation = waiting.generation;
            drop(waiting);
            publish_version(&self.generation, generation);
        }
    }
}

impl PermissionRuleCatalog for PermissionMemory {
    fn list(
        &self,
        principal: RulePrincipal,
    ) -> Result<Vec<ListedPermissionRule>, RuleCatalogError> {
        match self.evaluator.may(principal, RuleChange::List) {
            Ok(true) => {}
            Ok(false) => return Err(RuleCatalogError::Refused),
            Err(_) => return Err(RuleCatalogError::Unavailable),
        }
        let rules = lock(&self.rules);
        Ok(rules
            .rows
            .iter()
            .take(MAX_RULES)
            .map(|rule| ListedPermissionRule {
                id: rule.id.clone(),
                revision: rule.revision.clone(),
                workspace: rule.workspace.clone(),
                source: rule.source.clone(),
                tool: rule.tool.clone(),
                effect: match rule.effect {
                    PermissionEffect::Allow => "allow",
                    PermissionEffect::Deny => "deny",
                },
            })
            .collect())
    }

    fn revoke(
        &self,
        id: &str,
        principal: RulePrincipal,
        actor: &ActionContext,
    ) -> Result<bool, RuleCatalogError> {
        match self.evaluator.may(principal, RuleChange::Revoke) {
            Ok(true) => {}
            Ok(false) => return Err(RuleCatalogError::Refused),
            Err(_) => return Err(RuleCatalogError::Unavailable),
        }
        let mut rules = lock(&self.rules);
        let observed = self.clock.unix_milliseconds();
        let removed = rules
            .store
            .revoke(id, actor, observed)
            .map_err(|_| RuleCatalogError::Unavailable)?;
        if removed {
            rules.rows.retain(|rule| rule.id != id);
        }
        Ok(removed)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn replace_row(rows: &mut Vec<StoredPermissionRule>, stored: StoredPermissionRule) {
    if let Some(row) = rows.iter_mut().find(|row| row.id == stored.id) {
        *row = stored;
        return;
    }
    if let Some(index) = rows.iter().position(|row| {
        row.workspace == stored.workspace && row.source == stored.source && row.tool == stored.tool
    }) {
        rows[index] = stored;
        return;
    }
    rows.push(stored);
}

fn scope_key(conversation: &str, execution: &ExecutionId) -> String {
    format!("{conversation}\0{}", execution.as_str())
}

fn noted_key(conversation: &str, execution: &ExecutionId, request_id: &str) -> String {
    format!("{}\0{request_id}", scope_key(conversation, execution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::application::agent_execution::permissions::RememberRequest;
    use nessa_sdk::domain::agent_execution::executions::ExecutionId;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{
        sync::{mpsc as thread_channel, Barrier},
        thread,
        time::{Duration, Instant},
    };

    struct FixedClock {
        now: AtomicU64,
    }

    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }
    }

    fn clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock {
            now: AtomicU64::new(1_700_000_000_000),
        })
    }

    fn execution() -> ExecutionId {
        ExecutionId::new("turn-1").unwrap()
    }

    fn actor() -> ActionContext {
        ActionContext::new("owner", "surface", "request").unwrap()
    }

    fn open() -> (tempfile::TempDir, PermissionMemory) {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("rules");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("permission-rules.sqlite3");
        let memory = PermissionMemory::open(&path, clock()).unwrap();
        (directory, memory)
    }

    #[test]
    fn review_board_snapshot_owns_versions_before_notification() {
        for clearing in [false, true] {
            let (_directory, memory) = open();
            let notice = ReviewNotice {
                conversation_id: "conversation".into(),
                execution_id: execution(),
                permission_id: PermissionId::new("permission").unwrap(),
                tool: "Read".into(),
            };
            if clearing {
                memory.waiting(&notice);
            }
            let before = memory.current().0;
            let visibility = memory.visibility();
            thread::scope(|scope| {
                let memory = &memory;
                // This is the existing Tokio watch read guard, not a production
                // test hook. It blocks only projection writes, not board state.
                let projection = memory.generation.borrow();
                let writer = scope.spawn(|| {
                    if clearing {
                        memory.clear(
                            &notice.conversation_id,
                            &notice.execution_id,
                            &notice.permission_id,
                        );
                    } else {
                        memory.waiting(&notice);
                    }
                });
                let (send, receive) = thread_channel::sync_channel(1);
                let reader = scope.spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    loop {
                        let (generation, notices) = memory.current();
                        if notices.len() == usize::from(!clearing) {
                            send.send((generation, memory.is_current(before, visibility)))
                                .unwrap();
                            return;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "the actual board mutation must progress"
                        );
                        thread::yield_now();
                    }
                });
                let observed = receive.recv_timeout(Duration::from_secs(3));
                // Release even on an assertion failure before scoped joining.
                drop(projection);
                writer.join().unwrap();
                reader.join().unwrap();
                let (generation, old_current) = observed.expect(
                    "board snapshot must read authoritative state independently of wake projection",
                );
                assert_eq!(
                    generation,
                    before + 1,
                    "a snapshot generation must own its notice contents"
                );
                assert!(
                    !old_current,
                    "an earlier token must be invalid before notification catches up"
                );
                assert_eq!(*memory.generation.borrow(), generation);
            });
        }
    }

    #[test]
    fn review_board_concurrent_mutations_keep_monotonic_versions() {
        const WRITERS: usize = 8;
        const CHANGES: usize = 64;
        let (_directory, memory) = open();
        let start = Barrier::new(WRITERS);
        thread::scope(|scope| {
            let memory = &memory;
            let start = &start;
            for writer in 0..WRITERS {
                scope.spawn(move || {
                    start.wait();
                    for change in 0..CHANGES {
                        memory.waiting(&ReviewNotice {
                            conversation_id: format!("conversation-{writer}"),
                            execution_id: execution(),
                            permission_id: PermissionId::new(format!(
                                "permission-{writer}-{change}"
                            ))
                            .unwrap(),
                            tool: "Read".into(),
                        });
                        memory.recheck_visibility();
                    }
                });
            }
        });
        let expected = u64::try_from(WRITERS * CHANGES).unwrap();
        assert_eq!(
            memory.current().0,
            expected,
            "each actual notice mutation owns one version"
        );
        assert_eq!(
            memory.visibility(),
            expected,
            "concurrent visibility changes must not lose increments"
        );
        assert_eq!(*memory.generation.borrow(), expected);
        assert_eq!(*memory.visibility.borrow(), expected);
        assert!(memory.is_current(expected, expected));
    }

    #[tokio::test]
    async fn review_board_late_projection_cannot_regress_waiters() {
        let (_directory, memory) = open();
        for permission in ["first", "second"] {
            memory.waiting(&ReviewNotice {
                conversation_id: "conversation".into(),
                execution_id: execution(),
                permission_id: PermissionId::new(permission).unwrap(),
                tool: "Read".into(),
            });
        }
        assert_eq!(memory.current().0, 2);
        // A first writer can resume its projection after a second writer's
        // projection. Drive that delivery ordering through the actual projector,
        // then observe the public waiter rather than a helper return value.
        publish_version(&memory.generation, 1);
        assert_eq!(
            *memory.generation.borrow(),
            2,
            "a late projection must not move the wake version backwards"
        );
        // A projection may also lag the committed source before its writer is
        // scheduled. It is wake data, not the source comparison for a waiter.
        memory.generation.send_replace(1);
        let mut next = Box::pin(memory.wait_after(2));
        assert!(
            futures_util::poll!(next.as_mut()).is_pending(),
            "a late projection must not regress a current waiter"
        );
        memory.clear(
            "conversation",
            &execution(),
            &PermissionId::new("first").unwrap(),
        );
        assert!(futures_util::poll!(next.as_mut()).is_ready());
        assert_eq!(memory.current().0, 3);
    }

    #[test]
    fn review_board_exhausted_versions_refuse_candidate_validation() {
        let (_directory, memory) = open();
        assert!(memory.is_current(0, 0));
        lock(&memory.waiting).generation = u64::MAX;
        assert!(!memory.is_current(u64::MAX, 0));
        lock(&memory.waiting).generation = 0;
        lock(&memory.waiting).visibility = u64::MAX;
        assert!(!memory.is_current(0, u64::MAX));
    }

    fn remember(
        memory: &PermissionMemory,
        tool: &str,
        effect: PermissionEffect,
    ) -> Result<SettledRule, RememberError> {
        let execution = execution();
        memory.arm("conversation", &execution, true);
        memory.remember(RememberRequest {
            workspace: "/work",
            source: "claude",
            tool,
            effect,
            granted_by: &actor(),
            caller: PermissionCaller::Owner,
            execution_id: &execution,
            conversation_id: "conversation",
        })
    }

    #[test]
    fn a_grant_and_its_audit_commit_together() {
        let (_dir, memory) = open();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        let settled = memory
            .settle(PermissionCaller::Owner, "/work", "claude", "Bash")
            .unwrap();
        assert_eq!(settled.reference().rule_id(), stored.reference().rule_id());
        let rules = lock(&memory.rules);
        let audits = rules.store.audits().unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].cause, "grant");
        assert_eq!(audits[0].before_effect, "absent");
        assert_eq!(audits[0].after_effect, "allow");
        assert_eq!(audits[0].principal_id, "owner");
        assert_eq!(audits[0].request_id, "request");
        assert_eq!(audits[0].rule_id, stored.reference().rule_id());
    }

    #[test]
    fn a_failed_audit_leaves_no_rule() {
        let (_dir, memory) = open();
        lock(&memory.rules).store.fail_next_audit();
        assert_eq!(
            remember(&memory, "Bash", PermissionEffect::Allow).unwrap_err(),
            RememberError::Unavailable
        );
        let rules = lock(&memory.rules);
        assert!(rules.rows.is_empty());
        assert!(rules.store.audits().unwrap().is_empty());
        assert!(rules.store.load().unwrap().is_empty());
    }

    #[test]
    fn a_revoke_and_its_audit_commit_together() {
        let (_dir, memory) = open();
        let stored = remember(&memory, "Bash", PermissionEffect::Deny).unwrap();
        assert!(memory
            .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
            .unwrap());
        assert!(memory
            .settle(PermissionCaller::Owner, "/work", "claude", "Bash")
            .is_none());
        let rules = lock(&memory.rules);
        assert!(rules.rows.is_empty());
        let audits = rules.store.audits().unwrap();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[1].cause, "revoke");
        assert_eq!(audits[1].before_effect, "deny");
        assert_eq!(audits[1].after_effect, "absent");
        assert_eq!(audits[1].revision, stored.reference().revision());
    }

    #[test]
    fn a_failed_revoke_audit_leaves_the_rule() {
        let (_dir, memory) = open();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        lock(&memory.rules).store.fail_next_audit();
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
                .unwrap_err(),
            RuleCatalogError::Unavailable
        );
        drop(lock(&memory.rules));
        let settled = memory
            .settle(PermissionCaller::Grant, "/work", "claude", "Bash")
            .unwrap();
        assert_eq!(settled.reference().rule_id(), stored.reference().rule_id());
        let rules = lock(&memory.rules);
        assert_eq!(rules.store.audits().unwrap().len(), 1);
    }

    #[test]
    fn a_replacement_records_the_effect_it_replaced() {
        let (_dir, memory) = open();
        let first = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        let replaced = remember(&memory, "Bash", PermissionEffect::Deny).unwrap();
        assert_eq!(replaced.reference().rule_id(), first.reference().rule_id());
        assert_ne!(
            replaced.reference().revision(),
            first.reference().revision()
        );
        let audits = lock(&memory.rules).store.audits().unwrap();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[1].cause, "replace");
        assert_eq!(audits[1].before_effect, "allow");
        assert_eq!(audits[1].after_effect, "deny");
        assert_eq!(
            memory
                .settle(PermissionCaller::Owner, "/work", "claude", "Bash")
                .unwrap()
                .effect(),
            PermissionEffect::Deny
        );
    }

    #[test]
    fn a_grant_caller_cannot_remember_list_or_revoke() {
        let (_dir, memory) = open();
        let execution = execution();
        memory.arm("conversation", &execution, true);
        assert_eq!(
            memory
                .remember(RememberRequest {
                    workspace: "/work",
                    source: "claude",
                    tool: "Bash",
                    effect: PermissionEffect::Allow,
                    granted_by: &actor(),
                    caller: PermissionCaller::Grant,
                    execution_id: &execution,
                    conversation_id: "conversation",
                })
                .unwrap_err(),
            RememberError::Refused
        );
        assert_eq!(
            memory.list(RulePrincipal::Grant).unwrap_err(),
            RuleCatalogError::Refused
        );
        assert_eq!(
            memory
                .revoke("pr_missing", RulePrincipal::Grant, &actor())
                .unwrap_err(),
            RuleCatalogError::Refused
        );
        assert!(lock(&memory.rules).store.audits().unwrap().is_empty());
    }

    #[test]
    fn an_unarmed_turn_is_refused_and_a_reopen_keeps_the_rule() {
        let (dir, memory) = open();
        let execution = execution();
        memory.arm("conversation", &execution, false);
        assert_eq!(
            memory
                .remember(RememberRequest {
                    workspace: "/work",
                    source: "claude",
                    tool: "Bash",
                    effect: PermissionEffect::Allow,
                    granted_by: &actor(),
                    caller: PermissionCaller::Owner,
                    execution_id: &execution,
                    conversation_id: "conversation",
                })
                .unwrap_err(),
            RememberError::Refused
        );
        let stored = remember(&memory, "Read", PermissionEffect::Allow).unwrap();
        let path = dir.path().join("rules").join("permission-rules.sqlite3");
        let reopened = PermissionMemory::open(&path, clock()).unwrap();
        assert_eq!(
            reopened
                .settle(PermissionCaller::Owner, "/work", "claude", "Read")
                .unwrap()
                .reference()
                .rule_id(),
            stored.reference().rule_id()
        );
        assert_eq!(reopened.list(RulePrincipal::Owner).unwrap().len(), 1);
    }

    #[test]
    fn the_same_execution_id_in_another_conversation_is_not_this_turn() {
        let (_dir, memory) = open();
        let execution = execution();
        memory.arm("one", &execution, true);
        memory.arm("two", &execution, false);
        assert!(memory.offers_remember("one", &execution));
        assert!(!memory.offers_remember("two", &execution));
        assert_eq!(
            memory
                .remember(RememberRequest {
                    workspace: "/work",
                    source: "claude",
                    tool: "Bash",
                    effect: PermissionEffect::Allow,
                    granted_by: &actor(),
                    caller: PermissionCaller::Owner,
                    execution_id: &execution,
                    conversation_id: "two",
                })
                .unwrap_err(),
            RememberError::Refused
        );
    }

    #[test]
    fn the_two_hundred_fifty_seventh_new_key_is_not_stored() {
        let (_dir, memory) = open();
        for index in 0..MAX_RULES {
            remember(&memory, &format!("tool-{index}"), PermissionEffect::Allow).unwrap();
        }
        assert_eq!(
            remember(&memory, "overflow", PermissionEffect::Allow).unwrap_err(),
            RememberError::Unavailable
        );
        assert!(memory
            .settle(PermissionCaller::Owner, "/work", "claude", "overflow")
            .is_none());
        assert_eq!(memory.list(RulePrincipal::Owner).unwrap().len(), MAX_RULES);
    }

    #[test]
    fn a_row_that_is_not_a_rule_is_left_in_place() {
        let (directory, memory) = open();
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        lock(&memory.rules).store.insert_not_a_rule().unwrap();
        assert_eq!(
            remember(&memory, "Bash", PermissionEffect::Allow).unwrap_err(),
            RememberError::Unavailable
        );
        assert_eq!(
            memory
                .revoke("pr_not_a_rule", RulePrincipal::Owner, &actor())
                .unwrap_err(),
            RuleCatalogError::Unavailable
        );
        assert!(matches!(
            lock(&memory.rules).store.load().unwrap_err(),
            LoadError::NotARule
        ));
        assert!(lock(&memory.rules).store.audits().unwrap().is_empty());
        drop(memory);
        match PermissionMemory::open(&path, clock()) {
            Err(RulesOpenError::Store(OpenError::Damaged(reason))) => {
                assert!(reason.contains("not a rule"));
            }
            Err(RulesOpenError::Store(other)) => panic!("opened as {other}"),
            Err(RulesOpenError::Policy) => panic!("policy refused a stored row"),
            Ok(_) => panic!("opened a file that holds a row that is not a rule"),
        }
    }

    #[test]
    fn a_notice_carries_every_review_the_board_still_holds() {
        let (_dir, memory) = open();
        let execution = execution();
        for index in 0..100 {
            memory.waiting(&ReviewNotice {
                conversation_id: format!("conversation-{index}"),
                execution_id: execution.clone(),
                permission_id: PermissionId::new(format!("p{index}")).unwrap(),
                tool: "Read".into(),
            });
        }
        assert_eq!(memory.current().1.len(), 100);
        for index in 100..140 {
            memory.waiting(&ReviewNotice {
                conversation_id: format!("conversation-{index}"),
                execution_id: execution.clone(),
                permission_id: PermissionId::new(format!("p{index}")).unwrap(),
                tool: "Read".into(),
            });
        }
        let reviews = memory.current().1;
        assert_eq!(reviews.len(), 128);
        assert_eq!(reviews[0].permission_id, "p12");
        assert_eq!(reviews.last().unwrap().permission_id, "p139");
    }

    #[test]
    fn remember_and_revoke_leave_the_cache_agreeing_with_the_file() {
        let (directory, memory) = open();
        let memory = Arc::new(memory);
        let writer = {
            let memory = Arc::clone(&memory);
            std::thread::spawn(move || {
                for _ in 0..40 {
                    let _ = remember(&memory, "Bash", PermissionEffect::Allow);
                }
            })
        };
        let revoker = {
            let memory = Arc::clone(&memory);
            std::thread::spawn(move || {
                for _ in 0..40 {
                    if let Ok(listed) = memory.list(RulePrincipal::Owner) {
                        for rule in listed {
                            let _ = memory.revoke(&rule.id, RulePrincipal::Owner, &actor());
                        }
                    }
                }
            })
        };
        writer.join().unwrap();
        revoker.join().unwrap();
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        let reopened = PermissionMemory::open(&path, clock()).unwrap();
        assert_eq!(
            memory.list(RulePrincipal::Owner).unwrap(),
            reopened.list(RulePrincipal::Owner).unwrap()
        );
    }
}
