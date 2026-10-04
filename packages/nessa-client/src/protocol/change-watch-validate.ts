import {
  changeWatchIdPattern,
  maxChangeWatchIdBytes,
  ChangeWatchEndReason,
} from "../generated/product.js"
import type {
  ConversationWatchResult,
  ConversationChanged,
  ConversationWatchEnded,
} from "../generated/product.js"
import { validPositiveReadEpoch } from "./passive-read-validate.js"

const ID = new RegExp(changeWatchIdPattern)

/** An opaque connection identity; it never carries source progress or authority. */
export function validChangeWatchId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length <= maxChangeWatchIdBytes &&
    ID.test(value) &&
    validPositiveReadEpoch(value.slice(value.lastIndexOf("-") + 1))
  )
}

function exact(
  value: unknown,
  keys: readonly string[],
): value is Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key))
  )
}

export function validChanged(value: unknown): value is ConversationChanged {
  return exact(value, ["watchId"]) && validChangeWatchId(value.watchId)
}

export function validWatchEnded(value: unknown): value is ConversationWatchEnded {
  return (
    exact(value, ["watchId", "reason"]) &&
    validChangeWatchId(value.watchId) &&
    (value.reason === ChangeWatchEndReason.Closed ||
      value.reason === ChangeWatchEndReason.NotificationFailed)
  )
}

export function watchResult(value: unknown): ConversationWatchResult {
  if (!validChanged(value)) throw new TypeError("Invalid change watch response")
  return value
}
