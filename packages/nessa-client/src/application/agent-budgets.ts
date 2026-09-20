/**
 * The budgets the gateway spends before it can answer a command that opens an
 * agent: waiting for a freshly staged runtime to respond at all, then the rest
 * of the ACP handshake, then tearing a failed process down.
 *
 * These mirror `protocol/defaults/agent-startup-budgets.json`, which
 * `crates/nessa-server/src/composition/agent_budgets.rs` compiles into what the
 * gateway actually spends. They are written out here rather than imported
 * because this package stays self-contained — the same rule as
 * `NessaClient.defaultUrl`, which pins the dev port the same way — and
 * `agent-budgets.test.ts` checks them against that table, so the numbers still
 * cannot drift.
 */
const launchMs = 120_000
const startupMs = 45_000
const shutdownGraceMs = 3_000
const killTimeoutMs = 2_000

/** Cover for audit writes, the response write, and scheduling. */
const marginMs = 10_000

/** The longest one agent launch can take before the gateway can answer. */
const oneLaunchMs = launchMs + startupMs + shutdownGraceMs + killTimeoutMs

/**
 * Deadline for a conversation command that can open or restore an agent.
 *
 * A client that gives up before the gateway has finished failing deletes its
 * request, and the typed answer is dropped when it arrives: the caller is told
 * "Conversation command failed" for a failure the gateway had already
 * described exactly. So this outlasts the gateway's worst case, which is *two*
 * launches rather than one. A command can wait on a warm-up that is already
 * paying the operating system's first-execution scan, and if that warm-up
 * fails the command still opens its own provider afterwards. That pair is
 * sequential, so the budgets add.
 *
 * The two-launch case needs a badly broken machine — normally the warm-up
 * succeeds and the command's own open is warm, or no warm-up is in flight and
 * there is one launch. The number is what it is because truncating a
 * legitimate answer is the defect this exists to prevent, not because a launch
 * is expected to take that long.
 *
 * It is a backstop against a gateway that has stopped answering at all, and
 * deliberately not a second opinion about how long a launch may take. It raises
 * a shorter configured deadline for these commands rather than replacing a
 * longer one, so a caller who has already allowed more time keeps it.
 */
export const agentOperationTimeoutMs = oneLaunchMs * 2 + marginMs
