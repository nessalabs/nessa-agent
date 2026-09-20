import budgets from "../../../../protocol/defaults/agent-startup-budgets.json"

/**
 * Deadline for a conversation command that can open or restore an agent.
 *
 * The gateway spends its startup budget on the ACP handshake and then tears a
 * failed process down before it can answer, so this has to outlast both. A
 * client that gives up first deletes its request and drops the typed answer
 * when it arrives: the caller is told "Conversation command failed" for a
 * failure the gateway had already described exactly.
 *
 * `protocol/defaults/agent-startup-budgets.json` is the one table, and
 * `crates/nessa-server/src/composition/agent_budgets.rs` compiles the same
 * bytes into what the gateway actually spends, so the two cannot drift.
 *
 * This is a backstop against a gateway that has stopped answering at all. It
 * is deliberately not a second opinion about how long a launch may take.
 */
export const agentOperationTimeoutMs =
  budgets.agent.startupMs +
  budgets.agent.shutdownGraceMs +
  budgets.agent.killTimeoutMs +
  budgets.client.marginMs
