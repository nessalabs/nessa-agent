/**
 * Reading a real gateway's conversation view for `mcp-apps-gateway.mjs` and
 * `gateway-window.mjs`: which harness permission its setup answers, which of
 * the app's reviews a step's own action opened, how long that step waits for
 * the review (#474), and what a text-only turn said. Pure — the wait takes
 * its clock and its reads — so each is tested without a gateway.
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

/** The app's pending reviews in a conversation view. */
export const appPermissions = (view) =>
  view.permissions.filter((each) => each.origin.kind === "app")

/**
 * How long `mcp-apps-gateway.mjs` waits for the review an app's destructive
 * call opens (#474): the client's `callDeadlineMs` (`mcpAppCallTiming`,
 * from `x-mcpAppCallTiming`). `callTool` waits that long, and a review the
 * gateway opens after the host has given up is withdrawn with the caller.
 */
export const appReviewWaitMs = (timing) => timing.callDeadlineMs

/**
 * How long an output may stay empty before the wait stops (#474).
 * A call the app shows as `pending` waits `appReviewWaitMs`: that is how
 * long the host holds the call open. An output that never becomes pending
 * is a call that was not made, so no review of it can arrive late, and the
 * wait ends here — the same short alignment the window's first look uses —
 * rather than holding until the call deadline.
 */
export const appReviewUnstartedMs = 15_000

/**
 * What one read says about the review a step is waiting for (#474, R6–R8).
 * `reviews` are the app's pending reviews; `output` is what the app shows
 * for the call, when the step is watching it.
 *
 * | | `output` | a review not in `baseline` | |
 * | --- | --- | --- | --- |
 * | R6 | pending, empty, or unread | listed | `{ kind: "review" }` — that review, however long admission took |
 * | R7 | anything else | not listed | `{ kind: "answered" }` — the call ended with no review to answer |
 * | R8 | `pending`, or empty only while `appReviewUnstartedMs` has not passed | not listed | `{ kind: "wait" }` — admission may still be in front of the review |
 *
 * A review and an output that has left pending, in one read, is R6: the
 * review is there to answer. Empty past `appReviewUnstartedMs`, with the
 * call never shown pending, stops: there is no in-flight call to wait out.
 */
export function decideReviewWait({ reviews, output }, baseline) {
  const review = newReview(reviews, baseline)
  if (review) return { kind: "review", review }
  if (output !== undefined && output !== "" && output !== "pending")
    return { kind: "answered", output }
  return { kind: "wait" }
}

/** What one read of the view keeps, so a missed review can say what was there. */
const reviewSample = (view, output, elapsed) => ({
  ms: elapsed,
  transcriptState: view.transcriptState,
  output: output ?? null,
  permissions: view.permissions.map((each) => ({
    kind: each.origin.kind,
    tool: each.origin.kind === "app" ? each.origin.tool : each.toolName,
    permissionId: each.permissionId,
  })),
})

/** Samples whose transcript, output or permissions differ, and always the last. */
export function changedSamples(samples) {
  if (samples.length === 0) return []
  const signature = (sample) =>
    JSON.stringify([sample.transcriptState, sample.output, sample.permissions])
  const kept = [samples[0]]
  for (const sample of samples.slice(1)) {
    if (signature(sample) !== signature(kept.at(-1))) kept.push(sample)
  }
  const last = samples.at(-1)
  if (kept.at(-1) !== last) kept.push(last)
  return kept
}

/**
 * What a step says when the wait ends with no review (R8). `reads` is how
 * many were taken; `samples` is the distinct ones (`changedSamples`). A
 * review absent from every read is said so: it did not arrive late inside
 * this wait.
 */
export function reviewAbsentMessage(samples, reads) {
  const last = samples.at(-1)
  const states = [
    ...new Set(samples.map((sample) => sample.transcriptState ?? "unknown")),
  ]
  const outputs = [
    ...new Set(samples.map((sample) => sample.output).filter((output) => output)),
  ]
  const sawPending = samples.some((sample) => sample.output === "pending")
  const kinds = [
    ...new Set(samples.flatMap((sample) => sample.permissions.map((each) => each.kind))),
  ]
  const transcript =
    states.length === 1 && states[0] !== "complete" && states[0] !== "complete_empty"
      ? `transcript stayed ${states[0]}, so an open review is withheld from the view until it is confirmed`
      : `transcript: ${states.join(", ") || "none"}`
  return (
    "no review of the app's destructive call reached the conversation's permissions " +
    `in ${reads} ${reads === 1 ? "read" : "reads"} over ${last?.ms ?? 0} ms ` +
    `(never listed in this wait, not a late one; ${transcript}; ` +
    `${
      sawPending
        ? `call output: ${outputs.join(", ") || "unread"}`
        : "call output never showed pending, so no call was in flight for a review to be late"
    }; ` +
    `permission origins seen: ${kinds.join(", ") || "none"})`
  )
}

/**
 * Polls `read` until the step's review is in the view (R6), the call's
 * output leaves pending (R7), or the bound has passed with neither (R8).
 * The bound is `deadlineMs` once the app has shown the call as pending,
 * and `unstartedMs` until then. `pending`, when given, reads that output.
 * `sleep` and `now` are the clock, so a test can place a review after any
 * number of milliseconds.
 */
export async function waitForAppReview({
  read,
  baseline,
  deadlineMs,
  unstartedMs = appReviewUnstartedMs,
  pending,
  sleep,
  now = Date.now,
  pollMs = 250,
}) {
  if (!Number.isFinite(deadlineMs) || deadlineMs < 0)
    throw new Error("waitForAppReview needs a deadline")
  if (!Number.isFinite(unstartedMs) || unstartedMs < 0)
    throw new Error("waitForAppReview needs an unstarted bound")
  const start = now()
  const samples = []
  let sawPending = false
  for (;;) {
    const view = await read()
    const output = pending ? await pending() : undefined
    if (output === "pending") sawPending = true
    const elapsed = now() - start
    const decision = decideReviewWait({ reviews: appPermissions(view), output }, baseline)
    samples.push(reviewSample(view, output, elapsed))
    if (decision.kind !== "wait")
      return { ...decision, samples: changedSamples(samples), reads: samples.length }
    // No reader: the step is not watching the app's output, so only the
    // call deadline bounds the wait. A reader that has not seen pending
    // yet stops at the unstarted bound.
    const bound =
      pending === undefined || sawPending ? deadlineMs : Math.min(deadlineMs, unstartedMs)
    if (elapsed >= bound)
      return { kind: "absent", samples: changedSamples(samples), reads: samples.length }
    await sleep(Math.min(pollMs, Math.max(bound - elapsed, 0)))
  }
}

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

/**
 * Every executionId the view already lists, on a turn or still in the queue.
 * The message step records this before it clicks, and {@link messagesArrived}
 * treats an id in the set as an earlier engine's send.
 */
export function executionIds(view) {
  const ids = new Set()
  for (const message of view.messages) ids.add(message.executionId)
  for (const message of view.pending) ids.add(message.executionId)
  return ids
}

/**
 * Sends of `text` whose executionId is not in `earlier`. A turn lists the
 * text as `userText`; a queue entry lists it as `text`. The same executionId
 * in both is one send, and the turn is the one kept. The words are not the
 * identity: another engine on this conversation may already have sent them
 * (`mcp-apps-gateway.mjs`, message).
 */
export function messagesArrived(view, earlier, text) {
  const arrived = []
  const seen = new Set()
  const take = (message, messageText) => {
    const id = message.executionId
    if (messageText !== text || earlier.has(id) || seen.has(id)) return
    seen.add(id)
    arrived.push(message)
  }
  for (const message of view.messages) take(message, message.userText)
  for (const message of view.pending) take(message, message.text)
  return arrived
}
