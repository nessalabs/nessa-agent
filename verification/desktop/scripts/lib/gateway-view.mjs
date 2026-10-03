/**
 * Reading a real gateway's conversation view for `mcp-apps-gateway.mjs`:
 * which harness permission its setup answers, and which of the app's reviews
 * a step's own action opened. Pure, so both are tested without a gateway.
 */

/** A pending permission's identity in the view. */
export const permissionKey = ({ executionId, permissionId }) =>
  `${executionId}:${permissionId}`

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

/** The view's distinct calls of `tool` of `server` (design rows A5, A6). */
export function callsOf(view, server, tool) {
  const calls = new Map()
  for (const call of view.tools)
    if (isTool(call, server, tool)) calls.set(callKey(call), call)
  return [...calls.values()]
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
