/**
 * Reading a real gateway's conversation view for `mcp-apps-gateway.mjs` and
 * `gateway-window.mjs`: which harness permission its setup answers, which of
 * the app's reviews a step's own action opened, and what a text-only turn
 * said. Pure, so each is tested without a gateway.
 */
import { permissionKey } from "../../../../scripts/mcp-test-server/evidence.mjs"
import { CannotRun } from "./cli.mjs"

export { permissionKey }

/** A tool call's identity in the view. */
export const callKey = ({ executionId, toolId }) => `${executionId}:${toolId}`

/** The view's tool call `permission` asks about, if it lists it. */
const callOf = (view, permission) =>
  view.tools.find(
    (each) =>
      each.executionId === permission.executionId && each.toolId === permission.toolId,
  )

/** Whether `call` is `tool` of MCP server `server`. */
const isTool = (call, server, tool) =>
  call?.mcp?.server === server && call.mcp.tool === tool

/**
 * What setup does with the view's harness permissions when it admits exactly
 * one call of `tool` (design rows A1–A4, #384). `admitted` is the call key
 * already admitted, or `null`; `answered` the permission keys already
 * answered.
 *
 * Returns `{ allow, extra }`: `allow` is `{ permission, option, call }` for the
 * one permission to answer now, or `null`; `extra` is the key of another call
 * of `tool` asking while one is admitted, or `null`. Permissions for any other
 * tool, and those already answered, are left alone.
 */
export function admitOnce(view, admitted, answered, server, tool) {
  for (const permission of view.permissions) {
    if (permission.origin.kind !== "harness") continue
    if (answered.has(permissionKey(permission))) continue
    const call = callOf(view, permission)
    if (!isTool(call, server, tool)) continue
    const key = callKey(call)
    if (admitted !== null && admitted !== key) return { allow: null, extra: key }
    const option = permission.options.find((each) => each.effect === "allow")
    if (!option) continue
    return { allow: { permission, option, call: key }, extra: null }
  }
  return { allow: null, extra: null }
}

/** The view's distinct calls of `tool` of `server`, each as the view first lists it. */
export function callsOf(view, server, tool) {
  const calls = new Map()
  for (const call of view.tools)
    if (isTool(call, server, tool) && !calls.has(callKey(call)))
      calls.set(callKey(call), call)
  return [...calls.values()]
}

/**
 * What setup makes of the view once the turn has ended (design rows A5, A6):
 * `{ kind: "ready", call }` when the turn completed with exactly one call of
 * `tool`, completed, naming its `resourceUri`; `{ kind: "repeated", calls }`
 * when there was more than one; `{ kind: "unusable", call }` otherwise (`call`
 * is the one call, or `undefined`).
 */
export function setupOutcome(view, server, tool) {
  const calls = callsOf(view, server, tool)
  if (calls.length > 1) return { kind: "repeated", calls }
  const [call] = calls
  const turn = view.messages.at(-1)
  if (
    turn?.status === "completed" &&
    call?.status === "completed" &&
    call.mcp.resourceUri
  )
    return { kind: "ready", call }
  return { kind: "unusable", call }
}

/** The keys of the app's pending reviews: a step's baseline. */
export const reviewKeys = (reviews) => new Set(reviews.map(permissionKey))

/**
 * The first of the app's pending `reviews` not in `baseline` — the one the
 * step's own action opened (design rows R1–R4) — or `undefined`.
 */
export const newReview = (reviews, baseline) =>
  reviews.find((each) => !baseline.has(permissionKey(each)))

/** Whether `review` is still among the pending `reviews`. */
export const stillPending = (reviews, review) =>
  reviews.some((each) => permissionKey(each) === permissionKey(review))

/**
 * Text the window draws as itself: letters, digits, whitespace and plain
 * punctuation. What `inlineRuns` (`model/transcript.ts`) draws otherwise —
 * `` `code` `` and `**strong**` — is outside it, as is anything else.
 */
const plain = /^[\p{L}\p{N}\s.,:;!?'’"()-]*$/u

/**
 * What the view's last turn said, `{ user, reply }` with whitespace folded as
 * the check reads the page's — when its reply is text-only: every part a text
 * part (no tool, no local notice), and both texts plain. Only then does the
 * window draw each as its text alone, the reply's parts one after another
 * (`transcriptFrom` in `gateway-views.ts`, `message.tsx`, `RichText`). A
 * thought part is left out, as `transcriptFrom` leaves it out: the window
 * does not draw it. Any other turn, and an empty reply or none, is "could not
 * run": the check cannot say what the window should draw for it.
 */
export function lastTurn(view) {
  const turn = view.messages.at(-1)
  if (!turn) throw new CannotRun("the conversation holds no turn")
  const fold = (text) => text.replace(/\s+/g, " ").trim()
  const drawn = turn.parts.filter((part) => part.kind !== "thought")
  const others = drawn.filter((part) => part.kind !== "text").map((part) => part.kind)
  const user = fold(turn.userText)
  const reply = fold(drawn.map((part) => part.text ?? "").join(""))
  if (others.length > 0)
    throw new CannotRun(`the reply is not text-only: it has ${others.join(", ")} parts`)
  if (reply === "") throw new CannotRun("the reply is empty")
  for (const [who, text] of [
    ["person's message", user],
    ["reply", reply],
  ])
    if (!plain.test(text))
      throw new CannotRun(
        `the ${who} ${JSON.stringify(text.slice(0, 200))} is not plain text`,
      )
  return { user, reply }
}
