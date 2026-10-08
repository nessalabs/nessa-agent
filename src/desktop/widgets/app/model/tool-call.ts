/**
 * The tool call an app was made for, and what of it the app has been told
 * (MCP Apps, *Data Passing*). The rules of the spec, as one function over
 * what was sent and where the call is now:
 *
 * - `tool-input-partial` only while the input is incomplete, and never once
 *   `tool-input` is sent;
 * - `tool-input` at most once, with the complete arguments;
 * - `tool-result` at most once, and never before `tool-input`;
 * - `tool-cancelled` at most once, whatever was sent before it.
 *
 * A call that ended before the view was ready is held and told in that
 * order when it is (`toolNotifications` is asked again on `initialized`).
 * A result cannot come without its arguments: a finished phase holds them.
 * A running call may not yet: `tool-input` waits until they are known, and
 * a finished call whose view never carried any holds `{}` so the result
 * can follow.
 */
import { notify, type JsonObject, type Outgoing } from "./json-rpc"

/** Where a call is. */
export type CallPhase =
  /** The agent is still writing the arguments: the best reading of them so far. */
  | { readonly kind: "streaming"; readonly partial: JsonObject }
  | { readonly kind: "running"; readonly arguments?: JsonObject }
  /** Finished, with its `CallToolResult` — an error result included. */
  | { readonly kind: "done"; readonly arguments: JsonObject; readonly result: JsonObject }
  /** Cancelled, with its arguments if they were complete. */
  | {
      readonly kind: "cancelled"
      readonly arguments?: JsonObject
      readonly reason?: string
    }

/** A tool call that declared a UI, as the conversation holds it. */
export interface AppCall {
  /** The session the call was made in: the widget's origin, and where its app's calls go. */
  readonly sessionId: string
  /** The execution the call belongs to, and the call itself: the app's identity (`app-ref.ts`). */
  readonly executionId: string
  readonly toolId: string
  /** The tool's name: what the widget is called. */
  readonly tool: string
  /** The tool as the server described it (`Tool`), when known: the app's `toolInfo`. */
  readonly definition?: JsonObject
  /** The `ui://` resource the tool declared. */
  readonly resourceUri: string
  readonly phase: CallPhase
}

/**
 * Which view a call is drawn in: its session, its execution and tool ids, and
 * its resource. A call that differs in any of these is another view's — the
 * bridge does not tell it (`setCall`) and its place draws a new view
 * (`app-view.tsx`) — the one statement of it.
 */
export function callView(call: AppCall): string {
  return JSON.stringify([call.sessionId, call.executionId, call.toolId, call.resourceUri])
}

/** What the view has been told of its call. */
export interface CallTold {
  /** The last partial arguments sent, as JSON text, so the same one is not sent twice. */
  readonly partial?: string
  readonly input: boolean
  readonly end: boolean
}

export const nothingTold: CallTold = { input: false, end: false }

function completeArguments(phase: CallPhase): JsonObject | undefined {
  return phase.kind === "streaming" ? undefined : phase.arguments
}

/** What to send now, given what was told and where the call is; and what is told after. */
export function toolNotifications(
  told: CallTold,
  phase: CallPhase,
): { readonly send: readonly Outgoing[]; readonly told: CallTold } {
  const send: Outgoing[] = []
  let next = told
  if (next.end) return { send, told: next }
  if (!next.input && phase.kind === "streaming") {
    const text = JSON.stringify(phase.partial)
    if (text !== next.partial) {
      send.push(
        notify("ui/notifications/tool-input-partial", { arguments: phase.partial }),
      )
      next = { ...next, partial: text }
    }
  }
  const args = completeArguments(phase)
  if (!next.input && args) {
    send.push(notify("ui/notifications/tool-input", { arguments: args }))
    next = { ...next, input: true }
  }
  if (phase.kind === "done") {
    send.push(notify("ui/notifications/tool-result", phase.result))
    next = { ...next, end: true }
  }
  if (phase.kind === "cancelled") {
    send.push(
      notify(
        "ui/notifications/tool-cancelled",
        phase.reason === undefined ? {} : { reason: phase.reason },
      ),
    )
    next = { ...next, end: true }
  }
  return { send, told: next }
}
