//! The turn policies answered by Cedar (slice J4 of #707, ADR 803 step 4,
//! gate 18): which tools a turn may call, where it may run commands and
//! which programs, and what an MCP App may ask.
//!
//! Composition builds each evaluator once and injects it behind its
//! application port. Callers pass typed facts and get a typed decision that
//! names the policies behind it; Cedar stays in this directory. The three
//! share one bundle, one schema, and one way of asking (`request.rs`).
//!
//! ```text
//! composition ──▶ Cedar…PolicyEvaluator::new()
//!                     │ bundle.rs loads and strictly validates
//!                     ▼
//!               schema.json + tools.cedar + commands.cedar + apps.cedar
//!               (every policy has a unique @id)
//!
//! ToolPolicyEvaluator    ──▶ evaluator.rs          ──┐
//! CommandPolicyEvaluator ──▶ command_evaluator.rs  ──┼─▶ request.rs: entities + request
//! AppCallPolicyEvaluator ──▶ app_evaluator.rs      ──┘        │
//!                                                              ▼
//!                               Cedar Authorizer ──▶ Allow / Refuse with the deciding @ids
//! ```
//!
//! The facts are what is true, never a verdict.
//!
//! **Tools** (`tools.cedar`): the initiator (the owner, or a grant with its
//! preset's name), and the tool's name with who named it. Which names a
//! preset allows is written in `tools.cedar`; a grant's own allow and deny
//! patterns become policies scoped to that grant, built as Cedar JSON from
//! the stored text (`grant_rules.rs`), so a pattern is never read as policy
//! syntax. Each `*` in a pattern matches any run of characters, and every
//! other character is itself. A forbid wins over a permit, and a call no
//! policy permits is refused. When a grant's rules allow nothing by name,
//! every tool its deny patterns leave is permitted (`tool.rules.unlisted`).
//! A pattern matches a name from any source, so a deny list is not "no
//! shell": `Bash` does not deny the gateway's `run`. The checked-in ids are
//! `tool.owner`, `tool.reads`, `tool.edits` and `tool.preset.*`; a grant's
//! are `tool.rules.allow.<i>`, `tool.rules.deny.<i>` and
//! `tool.rules.unlisted`, where `<i>` is the pattern's place in its list.
//! An id under `tool.rules.` is never checked in.
//!
//! **Commands** (`commands.cedar`): the gateway as configured (its command
//! hosts, whether it has an allow-list, the programs allowed and denied,
//! built once from `CommandPolicy`), the host asked about, the initiator
//! with the hosts a grant names for itself, and the program as named with
//! its file name. The owner uses every configured host
//! (`command.owner_host`); a grant only a configured host it names itself,
//! and none when it names none (`command.grant_host`). With an allow-list
//! only a bare name on it runs (`command.unlisted_program`), and a denied
//! file name never does (`command.denied_program`).
//!
//! **MCP Apps** (`apps.cedar`): the app with the server whose tool call drew
//! it, the server it names, and the tool as that server listed it, with
//! whether its visibility includes apps. An app reaches its own server
//! (`app.own_server`) and no other (`app.other_server`), and calls its own
//! server's tools that are visible to apps (`app.visible_tool`) and no other
//! server's (`app.other_server_tool`). Whether it is an app at all, whether
//! the tool was listed, the arguments' size and a person's review of a
//! destructive tool stay in Rust: lookups, capacity and a prompt.
//!
//! More policy sets join this bundle: a set adds its actions and entity types
//! to `schema.json`, its policies as a file listed in `bundle.rs`, the id
//! prefix of any policies it builds from stored data to `bundle.rs`'s
//! reserved list, and its own port and evaluator beside `evaluator.rs`. Entity
//! types are shared: a set that needs another fact of a grant adds it to
//! `Nessa::Grant`, and `request.rs`'s `grant` builds the entity for every
//! evaluator with every fact its caller holds. `commandHosts` is optional and
//! present only when the command evaluator asks: a tool policy must never
//! read it, since a forbid over a missing attribute is skipped, not applied.
//! Every policy is validated with every other when the bundle loads.
//!
//! A bundle that does not validate, a grant's rules that do not, and any
//! evaluation error are `ToolPolicyUnavailable`, which every caller refuses:
//! Cedar skips a forbid whose condition errors, so an error never lets a
//! decision stand.

mod app_evaluator;
mod bundle;
mod command_evaluator;
mod evaluator;
mod grant_rules;
mod request;

pub use app_evaluator::CedarAppCallPolicyEvaluator;
pub use command_evaluator::CedarCommandPolicyEvaluator;
pub use evaluator::CedarToolPolicyEvaluator;

#[cfg(test)]
#[path = "../../../../../tests/conversation/tool_policy_bundle.rs"]
mod tests;
