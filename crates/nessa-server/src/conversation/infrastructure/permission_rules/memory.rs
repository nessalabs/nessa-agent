//! The live rules and the reviews waiting on a person.
//!
//! `settle` asks Cedar. A miss, or a policy that cannot be asked, asks the
//! person. `remember` writes only for a turn the service armed, and only when
//! Cedar permits the caller who was noted for that answer. The file write and
//! the cache update are one lock, so a revoke cannot land between them.
//! A remember stays provisional until `acknowledge`. `withdraw` removes that
//! revision, or restores the one a replacement wrote over. A revoke records
//! the request, deletes the row, and only then records that it is absent.

use nessa_local_database::OpenError;
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, PermissionCaller, PermissionRules, RememberError,
    RememberRequest, ReviewNotice, ReviewNotices, SettledRule,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::agent_execution::permissions::{PermissionEffect, PermissionId};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::watch;

use crate::conversation::application::{
    rule_principal, ListedPermissionRule, PermissionRuleCatalog, PermissionRulePolicyEvaluator,
    PermissionRuleRevocation, PermissionRuleRevocationAudit, RuleCatalogError, RuleChange,
    RulePrincipal, RuleSettlement, StoredPermissionRule,
};

use super::cedar::CedarPermissionRuleEvaluator;
use super::store::{RuleStore, WriteError};

const MAX_ARMS: usize = 1024;
const MAX_NOTICES: usize = 64;
/// How many waiting reviews one notice carries. The board keeps the same
/// number, so a replacement snapshot is every review still waiting. The wire
/// schema states the same bound.
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

struct Arms {
    order: VecDeque<String>,
    owner: HashMap<String, bool>,
}

struct NotedCallers {
    order: VecDeque<String>,
    callers: HashMap<String, PermissionCaller>,
}

/// The file and the rows a settle reads. One lock covers both, so a commit
/// and the cache update cannot be split by a revoke. `undo` is the row a
/// provisional revision replaced, keyed by that new revision, until the
/// selection is recorded or withdrawn.
struct LiveRules {
    store: RuleStore,
    rows: Vec<StoredPermissionRule>,
    undo: HashMap<String, Option<StoredPermissionRule>>,
    undo_order: VecDeque<String>,
}

/// Rules, who may be offered a remember choice, and the reviews still waiting.
pub struct PermissionMemory {
    rules: Mutex<LiveRules>,
    arms: Mutex<Arms>,
    noted: Mutex<NotedCallers>,
    notices: Mutex<Vec<PublishedReview>>,
    generation: watch::Sender<u64>,
    evaluator: CedarPermissionRuleEvaluator,
    revocations: Arc<dyn PermissionRuleRevocationAudit>,
    /// The next revoke deletes nothing. Tests use it to leave the request
    /// record in place while the row stays.
    #[cfg(test)]
    delete_fails: AtomicBool,
    /// The next withdraw leaves the provisional row stored.
    #[cfg(test)]
    withdraw_fails: AtomicBool,
}

impl PermissionMemory {
    /// Open the rules file, skipping rows that are not rules.
    ///
    /// `revocations` is where a successful revoke leaves its evidence. The
    /// file and the cache share one lock with that write.
    pub fn open(
        path: &Path,
        revocations: Arc<dyn PermissionRuleRevocationAudit>,
    ) -> Result<Self, OpenError> {
        let store = RuleStore::open(path)?;
        let rows = store.load().map_err(OpenError::from)?;
        let (generation, _) = watch::channel(0);
        Ok(Self {
            rules: Mutex::new(LiveRules {
                store,
                rows,
                undo: HashMap::new(),
                undo_order: VecDeque::new(),
            }),
            arms: Mutex::new(Arms {
                order: VecDeque::new(),
                owner: HashMap::new(),
            }),
            noted: Mutex::new(NotedCallers {
                order: VecDeque::new(),
                callers: HashMap::new(),
            }),
            notices: Mutex::new(Vec::new()),
            generation,
            evaluator: CedarPermissionRuleEvaluator,
            revocations,
            #[cfg(test)]
            delete_fails: AtomicBool::new(false),
            #[cfg(test)]
            withdraw_fails: AtomicBool::new(false),
        })
    }

    /// The next revoke records the request and leaves the row.
    #[cfg(test)]
    fn fail_next_delete(&self) {
        self.delete_fails.store(true, Ordering::SeqCst);
    }

    /// The next withdraw leaves the provisional revision stored.
    #[cfg(test)]
    fn fail_next_withdraw(&self) {
        self.withdraw_fails.store(true, Ordering::SeqCst);
    }

    /// Whether Cedar permits `principal` to make `change`.
    pub fn authorize(
        &self,
        principal: RulePrincipal,
        change: RuleChange,
    ) -> Result<(), RuleCatalogError> {
        self.permit(principal, change)
    }

    /// The generation and the waiting reviews a notice should carry.
    pub fn current(&self) -> (u64, Vec<PublishedReview>) {
        let generation = *self.generation.borrow();
        let notices = lock(&self.notices);
        let skip = notices.len().saturating_sub(PUBLISHED_REVIEWS);
        (generation, notices.iter().skip(skip).cloned().collect())
    }

    /// Wait until the generation is no longer `seen`.
    pub async fn wait_after(&self, seen: u64) {
        let mut updates = self.generation.subscribe();
        loop {
            if *updates.borrow() != seen {
                return;
            }
            if updates.changed().await.is_err() {
                return;
            }
        }
    }
}

impl PermissionRules for PermissionMemory {
    fn settle(&self, workspace: &str, source: &str, tool: &str) -> Option<SettledRule> {
        if workspace.is_empty() || tool.is_empty() || (source != "claude" && source != "codex") {
            return None;
        }
        let rules = lock(&self.rules)
            .rows
            .iter()
            .filter(|rule| !rule.provisional)
            .cloned()
            .collect::<Vec<_>>();
        let settlement = self
            .evaluator
            .settle(workspace, source, tool, &rules)
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

    fn offers_remember(&self, execution: &ExecutionId) -> bool {
        lock(&self.arms)
            .owner
            .get(execution.as_str())
            .copied()
            .unwrap_or(false)
    }

    fn arm(&self, execution: &ExecutionId, owner: bool) {
        let mut arms = lock(&self.arms);
        let id = execution.as_str().to_owned();
        if arms.owner.insert(id.clone(), owner).is_none() {
            arms.order.push_back(id);
            while arms.order.len() > MAX_ARMS {
                if let Some(old) = arms.order.pop_front() {
                    arms.owner.remove(&old);
                }
            }
        }
    }

    fn note_answerer(&self, execution: &ExecutionId, request_id: &str, caller: PermissionCaller) {
        let mut noted = lock(&self.noted);
        let key = noted_key(execution, request_id);
        if noted.callers.insert(key.clone(), caller).is_none() {
            noted.order.push_back(key);
            while noted.order.len() > MAX_ARMS {
                if let Some(old) = noted.order.pop_front() {
                    noted.callers.remove(&old);
                }
            }
        }
    }

    fn answerer(&self, execution: &ExecutionId, request_id: &str) -> Option<PermissionCaller> {
        lock(&self.noted)
            .callers
            .remove(&noted_key(execution, request_id))
    }

    fn remember(&self, request: RememberRequest<'_>) -> Result<SettledRule, RememberError> {
        if !self.offers_remember(request.execution_id)
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
        let previous = rules
            .rows
            .iter()
            .find(|rule| {
                rule.workspace == request.workspace
                    && rule.source == request.source
                    && rule.tool == request.tool
            })
            .cloned();
        let id = format!("pr_{}", uuid::Uuid::new_v4().simple());
        let stored = rules
            .store
            .remember(
                request.workspace,
                request.source,
                request.tool,
                request.effect,
                request.granted_by,
                id,
            )
            .map_err(|error| match error {
                WriteError::Database | WriteError::Revision => RememberError::Unavailable,
            })?;
        replace_row(&mut rules.rows, stored.clone());
        remember_undo(&mut rules, &stored, previous);
        let reference = ApprovalRuleReference::new(&stored.id, &stored.revision, stored.granted_by)
            .map_err(|_| RememberError::Unavailable)?;
        Ok(SettledRule::new(reference, stored.effect))
    }

    fn acknowledge(&self, rule: &ApprovalRuleReference) {
        let mut rules = lock(&self.rules);
        let acknowledged = rules
            .store
            .acknowledge(rule.rule_id(), rule.revision())
            .unwrap_or(false);
        if !acknowledged {
            return;
        }
        if let Some(row) = rules
            .rows
            .iter_mut()
            .find(|row| row.id == rule.rule_id() && row.revision == rule.revision())
        {
            row.provisional = false;
        }
        let key = undo_key(rule.rule_id(), rule.revision());
        rules.undo.remove(&key);
        rules.undo_order.retain(|item| item != &key);
    }

    fn withdraw(&self, rule: &ApprovalRuleReference) -> Result<(), RememberError> {
        #[cfg(test)]
        if self.withdraw_fails.swap(false, Ordering::SeqCst) {
            return Err(RememberError::Unavailable);
        }
        let mut rules = lock(&self.rules);
        let key = undo_key(rule.rule_id(), rule.revision());
        let Some(previous) = rules.undo.remove(&key) else {
            return Ok(());
        };
        rules.undo_order.retain(|item| item != &key);
        let Some(current) = rules
            .rows
            .iter()
            .find(|row| row.id == rule.rule_id())
            .cloned()
        else {
            return Ok(());
        };
        if current.revision != rule.revision() || !current.provisional {
            return Ok(());
        }
        match previous {
            Some(previous) => match rules.store.restore(&current.revision, &previous) {
                Ok(Some(restored)) => {
                    replace_row(&mut rules.rows, restored);
                    Ok(())
                }
                Ok(None) | Err(_) => {
                    rules.undo.insert(key, Some(previous));
                    Err(RememberError::Unavailable)
                }
            },
            None => match rules.store.delete_revision(&current.id, &current.revision) {
                Ok(true) => {
                    rules.rows.retain(|row| row.id != current.id);
                    Ok(())
                }
                Ok(false) | Err(_) => {
                    rules.undo.insert(key, None);
                    Err(RememberError::Unavailable)
                }
            },
        }
    }
}

impl ReviewNotices for PermissionMemory {
    fn waiting(&self, notice: &ReviewNotice) {
        let execution_id = notice.execution_id.as_str().to_owned();
        let permission_id = notice.permission_id.as_str().to_owned();
        let mut notices = lock(&self.notices);
        if let Some(existing) = notices
            .iter_mut()
            .find(|item| item.execution_id == execution_id && item.permission_id == permission_id)
        {
            if existing.conversation_id == notice.conversation_id && existing.tool == notice.tool {
                return;
            }
            existing.conversation_id.clone_from(&notice.conversation_id);
            existing.tool.clone_from(&notice.tool);
        } else {
            notices.push(PublishedReview {
                conversation_id: notice.conversation_id.clone(),
                execution_id,
                permission_id,
                tool: notice.tool.clone(),
            });
            while notices.len() > MAX_NOTICES {
                notices.remove(0);
            }
        }
        drop(notices);
        self.bump();
    }

    fn clear(&self, execution: &ExecutionId, permission: &PermissionId) {
        let mut notices = lock(&self.notices);
        let before = notices.len();
        notices.retain(|item| {
            item.execution_id != execution.as_str() || item.permission_id != permission.as_str()
        });
        let removed = notices.len() != before;
        drop(notices);
        if removed {
            self.bump();
        }
    }
}

impl PermissionRuleCatalog for PermissionMemory {
    fn list(
        &self,
        principal: RulePrincipal,
    ) -> Result<Vec<ListedPermissionRule>, RuleCatalogError> {
        self.permit(principal, RuleChange::List)?;
        Ok(lock(&self.rules)
            .rows
            .iter()
            .take(MAX_RULES)
            .map(listed)
            .collect())
    }

    fn revoke(
        &self,
        id: &str,
        principal: RulePrincipal,
        actor: &ActionContext,
    ) -> Result<bool, RuleCatalogError> {
        self.permit(principal, RuleChange::Revoke)?;
        let revocations = Arc::clone(&self.revocations);
        let mut rules = lock(&self.rules);
        let Some(existing) = rules.rows.iter().find(|rule| rule.id == id).cloned() else {
            drop(rules);
            return revocations.finish(id, actor);
        };
        let revocation = PermissionRuleRevocation {
            id: existing.id.clone(),
            revision: existing.revision.clone(),
            workspace: existing.workspace.clone(),
            source: existing.source.clone(),
            tool: existing.tool.clone(),
            effect: existing.effect,
            actor: actor.clone(),
        };
        // The request does not say the rule is gone. A failed delete leaves
        // this record, the row, and a retry that agrees.
        revocations.requested(&revocation)?;
        #[cfg(test)]
        if self.delete_fails.swap(false, Ordering::SeqCst) {
            return Err(RuleCatalogError::Unavailable);
        }
        rules
            .store
            .revoke(id)
            .map_err(|_| RuleCatalogError::Unavailable)?;
        rules.rows.retain(|rule| rule.id != id);
        drop(rules);
        // The absent record follows the delete. Losing it leaves the rule
        // gone; the retry finds no row and finishes the record.
        revocations.record(&revocation)?;
        Ok(true)
    }
}

impl PermissionMemory {
    fn permit(&self, principal: RulePrincipal, change: RuleChange) -> Result<(), RuleCatalogError> {
        match self.evaluator.may(principal, change) {
            Ok(true) => Ok(()),
            Ok(false) => Err(RuleCatalogError::Refused),
            Err(_) => Err(RuleCatalogError::Unavailable),
        }
    }

    fn bump(&self) {
        let next = self.generation.borrow().saturating_add(1);
        self.generation.send_replace(next);
    }
}

fn noted_key(execution: &ExecutionId, request_id: &str) -> String {
    format!("{}\0{request_id}", execution.as_str())
}

fn undo_key(id: &str, revision: &str) -> String {
    format!("{id}\0{revision}")
}

const MAX_UNDO: usize = 64;

fn remember_undo(
    rules: &mut LiveRules,
    stored: &StoredPermissionRule,
    previous: Option<StoredPermissionRule>,
) {
    let key = undo_key(&stored.id, &stored.revision);
    if rules.undo.insert(key.clone(), previous).is_none() {
        rules.undo_order.push_back(key);
    }
    while rules.undo_order.len() > MAX_UNDO {
        if let Some(old) = rules.undo_order.pop_front() {
            rules.undo.remove(&old);
        }
    }
}

fn replace_row(rows: &mut Vec<StoredPermissionRule>, stored: StoredPermissionRule) {
    if let Some(existing) = rows.iter_mut().find(|rule| {
        rule.workspace == stored.workspace
            && rule.source == stored.source
            && rule.tool == stored.tool
    }) {
        *existing = stored;
    } else {
        rows.push(stored);
    }
}

fn listed(rule: &StoredPermissionRule) -> ListedPermissionRule {
    ListedPermissionRule {
        id: rule.id.clone(),
        revision: rule.revision.clone(),
        workspace: rule.workspace.clone(),
        source: rule.source.clone(),
        tool: rule.tool.clone(),
        effect: match rule.effect {
            PermissionEffect::Allow => "allow",
            PermissionEffect::Deny => "deny",
        },
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::super::store::schema;
    use super::*;
    use nessa_sdk::application::agent_execution::permissions::{
        ActionContext, ApprovalRuleReference, RememberError, RememberRequest, ReviewNotice,
    };
    use nessa_sdk::domain::agent_execution::permissions::PermissionId;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RecordingRevocations {
        requested: Mutex<Vec<PermissionRuleRevocation>>,
        records: Mutex<Vec<PermissionRuleRevocation>>,
        /// Refuse the request record. The row is not deleted.
        fail: AtomicBool,
        /// Refuse the absent record. The row is already deleted.
        fail_removed: AtomicBool,
    }

    impl PermissionRuleRevocationAudit for RecordingRevocations {
        fn requested(&self, revocation: &PermissionRuleRevocation) -> Result<(), RuleCatalogError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(RuleCatalogError::Unavailable);
            }
            let mut requested = lock(&self.requested);
            if let Some(existing) = requested
                .iter()
                .find(|item| item.id == revocation.id && item.revision == revocation.revision)
            {
                if existing.actor.principal_id() != revocation.actor.principal_id() {
                    return Err(RuleCatalogError::Unavailable);
                }
                return Ok(());
            }
            requested.push(revocation.clone());
            Ok(())
        }

        fn record(&self, revocation: &PermissionRuleRevocation) -> Result<(), RuleCatalogError> {
            if self.fail_removed.load(Ordering::SeqCst) {
                return Err(RuleCatalogError::Unavailable);
            }
            let mut records = lock(&self.records);
            if records
                .iter()
                .any(|item| item.id == revocation.id && item.revision == revocation.revision)
            {
                return Ok(());
            }
            records.push(revocation.clone());
            Ok(())
        }

        fn finish(&self, id: &str, actor: &ActionContext) -> Result<bool, RuleCatalogError> {
            let requested = lock(&self.requested);
            let Some(started) = requested.iter().find(|item| item.id == id).cloned() else {
                return Ok(false);
            };
            if started.actor.principal_id() != actor.principal_id() {
                return Err(RuleCatalogError::Unavailable);
            }
            drop(requested);
            if lock(&self.records)
                .iter()
                .any(|item| item.id == id && item.revision == started.revision)
            {
                return Ok(false);
            }
            self.record(&started)?;
            Ok(true)
        }
    }

    fn opened() -> (
        tempfile::TempDir,
        PermissionMemory,
        Arc<RecordingRevocations>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("rules");
        nessa_local_storage::create_directory(&private).unwrap();
        let revocations = Arc::new(RecordingRevocations {
            requested: Mutex::new(Vec::new()),
            records: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            fail_removed: AtomicBool::new(false),
        });
        let memory = PermissionMemory::open(
            &private.join("permission-rules.sqlite3"),
            revocations.clone(),
        )
        .unwrap();
        (directory, memory, revocations)
    }

    fn execution(id: &str) -> ExecutionId {
        ExecutionId::new(id).unwrap()
    }

    fn actor() -> ActionContext {
        ActionContext::new("owner", "surface", "request").unwrap()
    }

    fn audit() -> Arc<RecordingRevocations> {
        Arc::new(RecordingRevocations {
            requested: Mutex::new(Vec::new()),
            records: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            fail_removed: AtomicBool::new(false),
        })
    }

    fn remember(
        memory: &PermissionMemory,
        tool: &str,
        effect: PermissionEffect,
    ) -> Result<SettledRule, RememberError> {
        let execution = execution("turn");
        memory.arm(&execution, true);
        memory.remember(RememberRequest {
            workspace: "/work",
            source: "claude",
            tool,
            effect,
            granted_by: &actor(),
            caller: PermissionCaller::Owner,
            execution_id: &execution,
        })
    }

    #[test]
    fn an_invalid_row_is_skipped_and_a_valid_rule_still_answers() {
        let (directory, _, _) = opened();
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        {
            let connection = nessa_local_database::open(&path, &schema()).unwrap();
            connection
                .execute(
                    "INSERT INTO permission_rule (
                        id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request, provisional
                     ) VALUES ('bad', '1', '/work', 'opencode', 'Bash', 'allow', 'owner', 'surface', 'request', 0)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO permission_rule (
                        id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request, provisional
                     ) VALUES ('good', '4', '/work', 'claude', 'Read', 'deny', 'owner', 'surface', 'request', 0)",
                    [],
                )
                .unwrap();
        }
        let memory = PermissionMemory::open(&path, audit()).unwrap();
        let listed = memory.list(RulePrincipal::Owner).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "good");
        assert_eq!(listed[0].effect, "deny");
        let settled = memory.settle("/work", "claude", "Read").unwrap();
        assert_eq!(settled.reference().rule_id(), "good");
        assert_eq!(settled.reference().revision(), "4");
        assert_eq!(settled.effect(), PermissionEffect::Deny);
        assert!(memory.settle("/work", "claude", "Bash").is_none());
    }

    #[test]
    fn a_remembered_allow_answers_the_next_review_and_a_replacement_bumps_the_revision() {
        let (_directory, memory, _) = opened();
        let first = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        assert_eq!(first.reference().revision(), "1");
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        memory.acknowledge(first.reference());
        let settled = memory.settle("/work", "claude", "Bash").unwrap();
        assert_eq!(settled.reference().rule_id(), first.reference().rule_id());
        assert_eq!(settled.effect(), PermissionEffect::Allow);
        assert!(memory.settle("/work", "claude", "Read").is_none());
        assert!(memory.settle("/other", "claude", "Bash").is_none());
        let replaced = remember(&memory, "Bash", PermissionEffect::Deny).unwrap();
        assert_eq!(replaced.reference().rule_id(), first.reference().rule_id());
        assert_eq!(replaced.reference().revision(), "2");
        // The replacement is provisional, so neither it nor the rule it
        // wrote over settles until the selection is recorded.
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        memory.acknowledge(replaced.reference());
        assert_eq!(
            memory.settle("/work", "claude", "Bash").unwrap().effect(),
            PermissionEffect::Deny
        );
    }

    #[test]
    fn a_shared_user_cannot_list_or_revoke_and_an_unarmed_turn_cannot_remember() {
        let (_directory, memory, revocations) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        assert_eq!(
            memory.list(RulePrincipal::Grant).unwrap_err(),
            RuleCatalogError::Refused
        );
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Grant, &actor())
                .unwrap_err(),
            RuleCatalogError::Refused
        );
        assert!(lock(&revocations.records).is_empty());
        assert!(memory
            .list(RulePrincipal::Owner)
            .unwrap()
            .iter()
            .any(|rule| rule.id == stored.reference().rule_id()));
        let other = execution("other");
        memory.arm(&other, false);
        let refused = memory.remember(RememberRequest {
            workspace: "/work",
            source: "claude",
            tool: "Read",
            effect: PermissionEffect::Allow,
            granted_by: &actor(),
            caller: PermissionCaller::Owner,
            execution_id: &other,
        });
        assert_eq!(refused.unwrap_err(), RememberError::Refused);
        assert!(memory
            .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
            .unwrap());
        let recorded = lock(&revocations.records);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].id, stored.reference().rule_id());
        assert_eq!(recorded[0].revision, stored.reference().revision());
        assert_eq!(recorded[0].effect, PermissionEffect::Allow);
        assert_eq!(recorded[0].workspace, "/work");
        assert_eq!(recorded[0].source, "claude");
        assert_eq!(recorded[0].tool, "Bash");
        assert_eq!(recorded[0].actor.principal_id(), "owner");
        assert_eq!(recorded[0].actor.request_id(), "request");
        drop(recorded);
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert!(!memory
            .revoke("missing", RulePrincipal::Owner, &actor())
            .unwrap());
        assert_eq!(lock(&revocations.records).len(), 1);
    }

    #[test]
    fn a_waiting_review_is_published_and_cleared_once() {
        let (_directory, memory, _) = opened();
        assert_eq!(memory.current().0, 0);
        let execution = execution("turn");
        let permission = PermissionId::new("perm").unwrap();
        memory.waiting(&ReviewNotice {
            conversation_id: "conversation".into(),
            execution_id: execution.clone(),
            permission_id: permission.clone(),
            tool: "Bash".into(),
        });
        let (generation, reviews) = memory.current();
        assert_eq!(generation, 1);
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].tool, "Bash");
        memory.waiting(&ReviewNotice {
            conversation_id: "conversation".into(),
            execution_id: execution.clone(),
            permission_id: permission.clone(),
            tool: "Bash".into(),
        });
        assert_eq!(memory.current().0, 1);
        memory.clear(&execution, &permission);
        assert!(memory.current().1.is_empty());
        assert_eq!(memory.current().0, 2);
        memory.clear(&execution, &permission);
        assert_eq!(memory.current().0, 2);
    }

    #[test]
    fn reopening_keeps_the_rule() {
        let (directory, memory, _) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.acknowledge(stored.reference());
        drop(memory);
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        let memory = PermissionMemory::open(&path, audit()).unwrap();
        assert_eq!(
            memory
                .settle("/work", "claude", "Bash")
                .unwrap()
                .reference()
                .rule_id(),
            stored.reference().rule_id()
        );
    }

    #[test]
    fn a_grant_caller_cannot_store_a_rule_on_an_owner_turn() {
        let (_directory, memory, _) = opened();
        let execution = execution("turn");
        memory.arm(&execution, true);
        let refused = memory.remember(RememberRequest {
            workspace: "/work",
            source: "claude",
            tool: "Read",
            effect: PermissionEffect::Allow,
            granted_by: &actor(),
            caller: PermissionCaller::Grant,
            execution_id: &execution,
        });
        assert_eq!(refused.unwrap_err(), RememberError::Refused);
        assert!(memory.list(RulePrincipal::Owner).unwrap().is_empty());
        assert!(memory.settle("/work", "claude", "Read").is_none());
    }

    #[test]
    fn a_failed_revocation_record_leaves_the_rule() {
        let (_directory, memory, revocations) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.acknowledge(stored.reference());
        revocations.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
                .unwrap_err(),
            RuleCatalogError::Unavailable
        );
        assert!(lock(&revocations.records).is_empty());
        assert_eq!(
            memory
                .settle("/work", "claude", "Bash")
                .unwrap()
                .reference()
                .rule_id(),
            stored.reference().rule_id()
        );
        assert!(lock(&revocations.requested).is_empty());
    }

    #[test]
    fn a_failed_delete_does_not_record_the_rule_as_gone_and_a_retry_removes_it() {
        let (_directory, memory, revocations) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.acknowledge(stored.reference());
        memory.fail_next_delete();
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
                .unwrap_err(),
            RuleCatalogError::Unavailable
        );
        assert_eq!(
            memory
                .settle("/work", "claude", "Bash")
                .unwrap()
                .reference()
                .revision(),
            "1"
        );
        assert!(lock(&revocations.records).is_empty());
        assert_eq!(lock(&revocations.requested).len(), 1);
        let retry = ActionContext::new("owner", "surface", "retry").unwrap();
        assert!(memory
            .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &retry)
            .unwrap());
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert_eq!(lock(&revocations.requested).len(), 1);
        assert_eq!(lock(&revocations.records).len(), 1);
        assert_eq!(lock(&revocations.records)[0].actor.request_id(), "retry");
    }

    #[test]
    fn a_lost_absent_record_is_finished_by_the_retry() {
        let (_directory, memory, revocations) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        revocations.fail_removed.store(true, Ordering::SeqCst);
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &actor())
                .unwrap_err(),
            RuleCatalogError::Unavailable
        );
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert!(lock(&revocations.records).is_empty());
        revocations.fail_removed.store(false, Ordering::SeqCst);
        let retry = ActionContext::new("owner", "surface", "retry").unwrap();
        assert!(memory
            .revoke(stored.reference().rule_id(), RulePrincipal::Owner, &retry)
            .unwrap());
        let records = lock(&revocations.records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].actor.request_id(), "request");
        assert_eq!(records[0].revision, stored.reference().revision());
    }

    #[test]
    fn a_rule_whose_selection_was_not_recorded_does_not_answer_a_later_review() {
        let (_directory, memory, revocations) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.withdraw(stored.reference()).unwrap();
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert!(memory.list(RulePrincipal::Owner).unwrap().is_empty());
        assert!(lock(&revocations.records).is_empty());
        assert!(lock(&revocations.requested).is_empty());
        let kept = remember(&memory, "Read", PermissionEffect::Allow).unwrap();
        memory.acknowledge(kept.reference());
        memory.withdraw(kept.reference()).unwrap();
        assert_eq!(
            memory
                .settle("/work", "claude", "Read")
                .unwrap()
                .reference()
                .rule_id(),
            kept.reference().rule_id()
        );
    }

    #[test]
    fn withdrawing_a_replacement_restores_the_previous_rule() {
        let (_directory, memory, _) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.acknowledge(stored.reference());
        let replaced = remember(&memory, "Bash", PermissionEffect::Deny).unwrap();
        assert_eq!(replaced.reference().revision(), "2");
        assert!(memory
            .remember(RememberRequest {
                workspace: "/work",
                source: "claude",
                tool: "Bash",
                effect: PermissionEffect::Allow,
                granted_by: &actor(),
                caller: PermissionCaller::Owner,
                execution_id: &execution("again"),
            })
            .is_err());
        memory.withdraw(replaced.reference()).unwrap();
        let settled = memory.settle("/work", "claude", "Bash").unwrap();
        assert_eq!(settled.reference().rule_id(), stored.reference().rule_id());
        assert_eq!(settled.reference().revision(), "3");
        assert_eq!(settled.effect(), PermissionEffect::Allow);
        memory.withdraw(stored.reference()).unwrap();
        assert_eq!(
            memory
                .settle("/work", "claude", "Bash")
                .unwrap()
                .reference()
                .revision(),
            "3"
        );
    }

    #[test]
    fn an_unaudited_revision_does_not_settle_after_a_failed_withdraw_or_a_reopen() {
        let (directory, memory, _) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        memory.fail_next_withdraw();
        assert_eq!(
            memory.withdraw(stored.reference()).unwrap_err(),
            RememberError::Unavailable
        );
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert_eq!(memory.list(RulePrincipal::Owner).unwrap().len(), 1);
        drop(memory);
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        let reopened = PermissionMemory::open(&path, audit()).unwrap();
        assert!(reopened.settle("/work", "claude", "Bash").is_none());
        assert_eq!(reopened.list(RulePrincipal::Owner).unwrap().len(), 1);
    }

    #[test]
    fn the_response_holds_at_most_256_rules_and_a_replacement_still_fits() {
        let (_directory, memory, _) = opened();
        for index in 0..256 {
            remember(&memory, &format!("t{index:03}"), PermissionEffect::Allow).unwrap();
        }
        let execution = execution("turn");
        let refused = memory.remember(RememberRequest {
            workspace: "/work",
            source: "claude",
            tool: "overflow",
            effect: PermissionEffect::Allow,
            granted_by: &actor(),
            caller: PermissionCaller::Owner,
            execution_id: &execution,
        });
        assert_eq!(refused.unwrap_err(), RememberError::Unavailable);
        assert_eq!(memory.list(RulePrincipal::Owner).unwrap().len(), 256);
        let current = memory
            .list(RulePrincipal::Owner)
            .unwrap()
            .into_iter()
            .find(|rule| rule.tool == "t000")
            .unwrap();
        memory.acknowledge(
            &ApprovalRuleReference::new(current.id, current.revision, actor()).unwrap(),
        );
        let replaced = remember(&memory, "t000", PermissionEffect::Deny).unwrap();
        assert_eq!(replaced.reference().revision(), "2");
        assert_eq!(memory.list(RulePrincipal::Owner).unwrap().len(), 256);
        assert!(memory.settle("/work", "claude", "overflow").is_none());
    }

    #[test]
    fn a_notice_carries_every_review_the_board_still_holds() {
        let (_directory, memory, _) = opened();
        let execution = execution("turn");
        for index in 0..40 {
            memory.waiting(&ReviewNotice {
                conversation_id: format!("conversation-{index}"),
                execution_id: execution.clone(),
                permission_id: PermissionId::new(format!("p{index}")).unwrap(),
                tool: "Read".into(),
            });
        }
        assert_eq!(memory.current().1.len(), 40);
        for index in 40..70 {
            memory.waiting(&ReviewNotice {
                conversation_id: format!("conversation-{index}"),
                execution_id: execution.clone(),
                permission_id: PermissionId::new(format!("p{index}")).unwrap(),
                tool: "Read".into(),
            });
        }
        let reviews = memory.current().1;
        assert_eq!(reviews.len(), 64);
        assert_eq!(reviews[0].permission_id, "p6");
        assert_eq!(reviews.last().unwrap().permission_id, "p69");
    }

    #[test]
    fn remember_and_revoke_leave_the_cache_agreeing_with_the_file() {
        let (directory, memory, _) = opened();
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
        let reopened = PermissionMemory::open(&path, audit()).unwrap();
        assert_eq!(
            memory.list(RulePrincipal::Owner).unwrap(),
            reopened.list(RulePrincipal::Owner).unwrap()
        );
    }
}
