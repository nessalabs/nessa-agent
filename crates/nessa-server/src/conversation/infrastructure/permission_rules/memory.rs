//! The live rules and the reviews waiting on a person.
//!
//! `settle` asks Cedar. A miss, or a policy that cannot be asked, asks the
//! person. `remember` writes only for an owner turn the service armed, and
//! only when Cedar permits the owner to grant a rule.

use nessa_local_database::OpenError;
use nessa_sdk::application::agent_execution::permissions::{
    ApprovalRuleReference, PermissionRules, RememberError, RememberRequest, ReviewNotice,
    ReviewNotices, SettledRule,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::agent_execution::permissions::{PermissionEffect, PermissionId};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tokio::sync::watch;

use crate::conversation::application::{
    ListedPermissionRule, PermissionRuleCatalog, PermissionRulePolicyEvaluator, RuleCatalogError,
    RuleChange, RulePrincipal, RuleSettlement, StoredPermissionRule,
};

use super::cedar::CedarPermissionRuleEvaluator;
use super::store::{RuleStore, WriteError};

const MAX_ARMS: usize = 1024;
const MAX_NOTICES: usize = 64;
/// How many waiting reviews one notice carries. The wire schema states the same bound.
const PUBLISHED_REVIEWS: usize = 32;

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

/// Rules, who may be offered a remember choice, and the reviews still waiting.
pub struct PermissionMemory {
    store: Mutex<RuleStore>,
    rows: Mutex<Vec<StoredPermissionRule>>,
    arms: Mutex<Arms>,
    notices: Mutex<Vec<PublishedReview>>,
    generation: watch::Sender<u64>,
    evaluator: CedarPermissionRuleEvaluator,
}

impl PermissionMemory {
    /// Open the rules file, skipping rows that are not rules.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let store = RuleStore::open(path)?;
        let rows = store.load().map_err(OpenError::from)?;
        let (generation, _) = watch::channel(0);
        Ok(Self {
            store: Mutex::new(store),
            rows: Mutex::new(rows),
            arms: Mutex::new(Arms {
                order: VecDeque::new(),
                owner: HashMap::new(),
            }),
            notices: Mutex::new(Vec::new()),
            generation,
            evaluator: CedarPermissionRuleEvaluator,
        })
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
        let rules = lock(&self.rows).clone();
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

    fn remember(&self, request: RememberRequest<'_>) -> Result<SettledRule, RememberError> {
        if !self.offers_remember(request.execution_id)
            || request.workspace.is_empty()
            || request.tool.is_empty()
            || (request.source != "claude" && request.source != "codex")
        {
            return Err(RememberError::Refused);
        }
        match self.evaluator.may(RulePrincipal::Owner, RuleChange::Grant) {
            Ok(true) => {}
            Ok(false) => return Err(RememberError::Refused),
            Err(_) => return Err(RememberError::Unavailable),
        }
        let id = format!("pr_{}", uuid::Uuid::new_v4().simple());
        let stored = {
            let mut store = lock(&self.store);
            store
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
                })?
        };
        replace_row(&mut lock(&self.rows), stored.clone());
        let reference = ApprovalRuleReference::new(&stored.id, &stored.revision, stored.granted_by)
            .map_err(|_| RememberError::Unavailable)?;
        Ok(SettledRule::new(reference, stored.effect))
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
        Ok(lock(&self.rows).iter().map(listed).collect())
    }

    fn revoke(&self, id: &str, principal: RulePrincipal) -> Result<bool, RuleCatalogError> {
        self.permit(principal, RuleChange::Revoke)?;
        let removed = lock(&self.store)
            .revoke(id)
            .map_err(|_| RuleCatalogError::Unavailable)?;
        if removed {
            lock(&self.rows).retain(|rule| rule.id != id);
        }
        Ok(removed)
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
        ActionContext, RememberError, RememberRequest, ReviewNotice,
    };
    use nessa_sdk::domain::agent_execution::permissions::PermissionId;

    fn opened() -> (tempfile::TempDir, PermissionMemory) {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("rules");
        nessa_local_storage::create_directory(&private).unwrap();
        let memory = PermissionMemory::open(&private.join("permission-rules.sqlite3")).unwrap();
        (directory, memory)
    }

    fn execution(id: &str) -> ExecutionId {
        ExecutionId::new(id).unwrap()
    }

    fn actor() -> ActionContext {
        ActionContext::new("owner", "surface", "request").unwrap()
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
            execution_id: &execution,
        })
    }

    #[test]
    fn an_invalid_row_is_skipped_and_a_valid_rule_still_answers() {
        let (directory, _) = opened();
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
                        granted_principal, granted_surface, granted_request
                     ) VALUES ('bad', '1', '/work', 'opencode', 'Bash', 'allow', 'owner', 'surface', 'request')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO permission_rule (
                        id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request
                     ) VALUES ('good', '4', '/work', 'claude', 'Read', 'deny', 'owner', 'surface', 'request')",
                    [],
                )
                .unwrap();
        }
        let memory = PermissionMemory::open(&path).unwrap();
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
        let (_directory, memory) = opened();
        let first = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        assert_eq!(first.reference().revision(), "1");
        let settled = memory.settle("/work", "claude", "Bash").unwrap();
        assert_eq!(settled.reference().rule_id(), first.reference().rule_id());
        assert_eq!(settled.effect(), PermissionEffect::Allow);
        assert!(memory.settle("/work", "claude", "Read").is_none());
        assert!(memory.settle("/other", "claude", "Bash").is_none());
        let replaced = remember(&memory, "Bash", PermissionEffect::Deny).unwrap();
        assert_eq!(replaced.reference().rule_id(), first.reference().rule_id());
        assert_eq!(replaced.reference().revision(), "2");
        assert_eq!(
            memory.settle("/work", "claude", "Bash").unwrap().effect(),
            PermissionEffect::Deny
        );
    }

    #[test]
    fn a_shared_user_cannot_list_or_revoke_and_an_unarmed_turn_cannot_remember() {
        let (_directory, memory) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        assert_eq!(
            memory.list(RulePrincipal::Grant).unwrap_err(),
            RuleCatalogError::Refused
        );
        assert_eq!(
            memory
                .revoke(stored.reference().rule_id(), RulePrincipal::Grant)
                .unwrap_err(),
            RuleCatalogError::Refused
        );
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
            execution_id: &other,
        });
        assert_eq!(refused.unwrap_err(), RememberError::Refused);
        assert!(memory
            .revoke(stored.reference().rule_id(), RulePrincipal::Owner)
            .unwrap());
        assert!(memory.settle("/work", "claude", "Bash").is_none());
        assert!(!memory.revoke("missing", RulePrincipal::Owner).unwrap());
    }

    #[test]
    fn a_waiting_review_is_published_and_cleared_once() {
        let (_directory, memory) = opened();
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
        let (directory, memory) = opened();
        let stored = remember(&memory, "Bash", PermissionEffect::Allow).unwrap();
        drop(memory);
        let path = directory
            .path()
            .join("rules")
            .join("permission-rules.sqlite3");
        let memory = PermissionMemory::open(&path).unwrap();
        assert_eq!(
            memory
                .settle("/work", "claude", "Bash")
                .unwrap()
                .reference()
                .rule_id(),
            stored.reference().rule_id()
        );
    }
}
