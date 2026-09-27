/**
 * How long the client waits for a conversation command that waits on an agent.
 *
 * Mirrors `protocol/defaults/agent-startup-budgets.json`. The gateway's own
 * test ties the table's `deletion.worstCaseMs` to what it actually spends —
 * stopping the conversation, waiting for its history, then the SDK's one
 * statement of an agent exchange (`AcpConfig::session_deletion_limit`) — so
 * that number has one owner, and this states it rather than adding its parts
 * again. Written out rather than imported because this package stays
 * self-contained — the same rule as `NessaClient.defaultUrl` — and
 * `agent-budgets.test.ts` checks both numbers against the table.
 */
const deletionWorstCaseMs = 233_000

/** Cover for audit writes, the response write, and scheduling. */
const marginMs = 10_000

/**
 * Deadline for `conversation.delete`, the one command that waits for an agent.
 *
 * A client that gives up before the gateway has finished deletes its request,
 * and the typed answer is dropped when it arrives: the caller is told the
 * command failed when the gateway had said exactly how it went. So this
 * outlasts the gateway's worst case for one attempt, which is all one delete
 * makes — a delete that waits behind another answers from what that one
 * recorded. It is a backstop against a gateway that has stopped answering, not
 * a guess at how long a delete takes: normally the agent is warm and a delete
 * answers in seconds.
 */
export const conversationDeleteTimeoutMs = deletionWorstCaseMs + marginMs
