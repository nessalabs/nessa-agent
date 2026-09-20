/**
 * The longest the gateway can legitimately spend on one agent launch before it
 * is able to answer: the ACP handshake, then tearing a failed process down.
 *
 * These mirror `protocol/defaults/agent-startup-budgets.json`, which
 * `crates/nessa-server/src/composition/agent_budgets.rs` compiles into what the
 * gateway actually spends. They are written out here rather than imported
 * because this package stays self-contained — the same rule as
 * `NessaClient.defaultUrl`, which pins the dev port the same way — and
 * `agent-budgets.test.ts` checks them against that table, so the numbers still
 * cannot drift.
 */
const startupMs = 45_000
const shutdownGraceMs = 3_000
const killTimeoutMs = 2_000

/** Cover for audit writes, the response write, and scheduling. */
const marginMs = 10_000

/**
 * Deadline for a conversation command that can open or restore an agent.
 *
 * A client that gives up before the gateway has finished failing deletes its
 * request, and the typed answer is dropped when it arrives: the caller is told
 * "Conversation command failed" for a failure the gateway had already
 * described exactly. So this outlasts the gateway's worst case.
 *
 * It is a backstop against a gateway that has stopped answering at all, and
 * deliberately not a second opinion about how long a launch may take. It raises
 * a shorter configured deadline for these commands rather than replacing a
 * longer one, so a caller who has already allowed more time keeps it.
 */
export const agentOperationTimeoutMs =
  startupMs + shutdownGraceMs + killTimeoutMs + marginMs
