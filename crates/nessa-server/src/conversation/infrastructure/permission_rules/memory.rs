//! The live remembered permission rules.
//!
//! `settle` asks Cedar as the caller the host passes. A miss, or a policy
//! that cannot be asked, asks the person. `remember` and `revoke` write the
//! rule and its audit row in one database transaction. Cedar admits or
//! refuses the caller; the owner is not assumed. A failure leaves the file
//! as it was.

use nessa_auth::application::ports::Clock;
use nessa_local_database::OpenError;
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, PermissionCaller, PermissionRules, RememberError,
    RememberRequest, SettledRule,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::conversation::application::{
    rule_principal, ListedPermissionRule, PermissionRuleCatalog, PermissionRulePolicyEvaluator,
    RuleCatalogError, RuleChange, RulePrincipal, RuleSettlement, StoredPermissionRule,
};

use super::cedar::CedarPermissionRuleEvaluator;
use super::store::{RememberWrite, RuleStore, WriteError};

const MAX_ARMS: usize = 1024;

/// Why the rules file was not opened.
#[derive(Debug)]
pub enum RulesOpenError {
    /// The SQLite file was not opened.
    Store(OpenError),
    /// The Cedar schema or policies did not validate.
    Policy,
}
/// How many rules `conversation.permissionRules` may return. The wire schema
/// states the same bound. A new key past it is not stored; replacing a key is.
const MAX_RULES: usize = 256;

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

/// Rules and who may be offered a remember choice.
pub struct PermissionMemory {
    rules: Mutex<LiveRules>,
    arms: Mutex<Arms>,
    noted: Mutex<NotedCallers>,
    evaluator: CedarPermissionRuleEvaluator,
    clock: Arc<dyn Clock>,
}

impl PermissionMemory {
    /// Open the rules file, skipping rows that are not rules.
    ///
    /// `clock` dates an audit row. It is the time this process observed the
    /// commit, not a time the caller claimed.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self, RulesOpenError> {
        let evaluator =
            CedarPermissionRuleEvaluator::validated().map_err(|_| RulesOpenError::Policy)?;
        let store = RuleStore::open(path).map_err(RulesOpenError::Store)?;
        let rows = store.load().unwrap_or_default();
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

    /// Whether Cedar permits `principal` to make `change`. A policy error
    /// is a refusal: a change that cannot be authorized is not made.
    pub fn permits(&self, principal: RulePrincipal, change: RuleChange) -> bool {
        self.authorize(principal, change).is_ok()
    }
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
        if !self.offers_remember(request.conversation, request.execution_id)
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
    format!("{conversation}\u{1f}{}", execution.as_str())
}

fn noted_key(conversation: &str, execution: &ExecutionId, request_id: &str) -> String {
    format!("{}\u{1f}{request_id}", scope_key(conversation, execution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::application::agent_execution::permissions::RememberRequest;
    use nessa_sdk::domain::agent_execution::executions::ExecutionId;
    use std::sync::atomic::{AtomicU64, Ordering};

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
            conversation: "conversation",
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
                    conversation: "conversation",
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
                    conversation: "conversation",
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
                    conversation: "two",
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
}
