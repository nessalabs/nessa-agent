//! Curated SEP-2640 Agent Skills shipped by `nessa mcp` so external agents
//! connecting to the gateway surface catalog know how to drive Nessa.
use nessa_sdk::domain::mcp_skills::{SkillCatalog, SkillDefinition, SkillFile};
use std::sync::OnceLock;

/// The static SEP-2640 skill catalog served by `nessa mcp`.
pub fn skills() -> &'static SkillCatalog {
    static CATALOG: OnceLock<SkillCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: "drive-nessa".to_owned(),
            files: vec![
                SkillFile::markdown(
                    "SKILL.md",
                    "---\n\
name: drive-nessa\n\
description: Drive Nessa conversations, agent turns, workspace layouts, and gateway configuration over the nessa mcp surface catalog.\n\
---\n\n\
# Driving Nessa over MCP\n\n\
Each tool exposed by `nessa mcp` maps one-to-one to a Nessa gateway product method. Every call authenticates with this process's `nessa-mcp` credential and is authorized by Cedar on the gateway.\n\n\
## Quick start\n\n\
1. Discover existing threads with `conversation.list` or create a new one with `conversation.create`.\n\
2. Send a prompt with `conversation.send` (or guide an active turn with `conversation.steer`), inspect progress with `conversation.read` or `conversation.peek`, and halt runaway execution with `conversation.stop` or `conversation.cancel`.\n\
3. Handle pending approvals and questions via `conversation.permissionReview`, `conversation.questionReview`, and `conversation.answerQuestion`.\n\
4. Read `references/conversations.md` for turn lifecycle and review patterns, and `references/workspace-and-tools.md` for workspace layouts and MCP management.\n",
                ),
                SkillFile::markdown(
                    "references/conversations.md",
                    "# Conversation lifecycle on nessa mcp\n\n\
## Creating and running turns\n\
- `conversation.create` opens a conversation and returns its identifier.\n\
- `conversation.send` submits a user message; `conversation.receipt` and `conversation.messageStatus` report delivery state.\n\
- `conversation.read` and `conversation.readEarlier` paginate transcript history; `conversation.recordsHead` / `conversation.recordsPage` inspect the underlying record stream.\n\n\
## Reviews, approvals, and lifecycle\n\
- Use `conversation.setApprovalMode` or `conversation.setModel` to adjust execution settings.\n\
- Resolve pending reviews with `conversation.permissionReview` or `conversation.answerQuestion`.\n\
- Clean up finished conversations with `conversation.close`, `conversation.archive`, or `conversation.delete`.\n",
                ),
                SkillFile::markdown(
                    "references/workspace-and-tools.md",
                    "# Workspaces and MCP controls on nessa mcp\n\n\
## Desktop workspace layout\n\
- Use `workspace.publish`, `workspace.arrange`, and `workspace.closePane` to organize open conversation panes.\n\n\
## Authorization and safety\n\
- Cedar on the gateway decides every call; a `forbidden` refusal means the current `nessa-mcp` token lacks the grant named in the tool description.\n\
- Prefer read-only methods (`readOnlyHint: true`) to inspect state before calling destructive methods (`destructiveHint: true`).\n",
                ),
            ],
        }])
        .expect("nessa mcp skill catalog is valid")
    })
}
