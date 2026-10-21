import type {
  ConversationPeekResult,
  ConversationListResult,
  ConversationMessageReceipt,
  ConversationMessageStatusResult,
  ConversationObserveResult,
  ConversationSummary,
  ConversationView,
  ConversationPermission,
  ConversationQuestion,
  ConversationLease,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationReorderResult,
  ConversationSetApprovalModeResult,
  ConversationSetUsageLimitAutoContinueResult,
  ConversationSetModelResult,
  ConversationShareResult,
  ConversationSharesResult,
  ApprovalMode,
  ImageAttachment,
  LinkedFile,
} from "../generated/product.js"
import {
  bounds,
  CompactionReportingSupport,
  ConversationPermissionAsk,
  ConversationPermissionOptionEffect,
  ConversationCapability,
  ConversationToolPreset,
  ElicitationForwardingSupport,
  IncomingElicitationSupport,
  ModelSwitchReportingSupport,
  NativeHookSuppressionSupport,
  NativeHooksChoice,
  PermissionDeferralSupport,
  PermissionDenialSupport,
  PolicyCloseSessionSupport,
  PolicyEndTurnSupport,
  PreToolPolicySupport,
  transcriptStates,
} from "../generated/product.js"
import {
  imageAttachments,
  linkedFiles,
  MAX_UPLOAD_BYTES,
  validDigest,
  validMediaType,
  validSize,
} from "./attachment-validate.js"
import { approvalModeChoices } from "./agents-validate.js"
import { boundedName } from "./mcp-app-validate.js"
import { canonicalBase64, decimal } from "./passive-read-validate.js"

const utf8 = new TextEncoder()
const maxTranscriptStateBytes = Math.max(
  ...transcriptStates.map((state) => utf8.encode(state).byteLength),
)

/** A canonical lowercase hyphenated UUID: the schema's pattern for a conversation identity. */
export const conversationIdPattern = new RegExp(bounds.conversationIdPattern)

/** A pending review's deadline: the wait, when it is due, the outcome, and whether that refusal stops the turn. */
function deadlineOf(value: unknown) {
  const deadline = record(value)
  exact(deadline, ["waitMs", "dueAtMs", "effect", "reason", "stopsTurn"])
  const waitMs = deadline.waitMs
  if (
    typeof waitMs !== "number" ||
    !Number.isInteger(waitMs) ||
    waitMs < 1 ||
    waitMs > 86_400_000
  )
    throw new Error("Invalid conversation waitMs")
  const dueAtMs = deadline.dueAtMs
  if (typeof dueAtMs !== "number" || !Number.isSafeInteger(dueAtMs) || dueAtMs < 1)
    throw new Error("Invalid conversation dueAtMs")
  const effect = text(deadline, "effect", 16)
  oneOf(effect, Object.values(ConversationPermissionOptionEffect))
  const reason = text(deadline, "reason", 512, false)
  if (!reason.trim() || /\p{Cc}/u.test(reason))
    throw new Error("Invalid conversation reason")
  flag(deadline, "stopsTurn")
  if (effect === ConversationPermissionOptionEffect.Allow && deadline.stopsTurn === true)
    throw new Error("Invalid conversation stopsTurn")
}
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid conversation response")
  return value as Record<string, unknown>
}
function exact(item: Record<string, unknown>, keys: readonly string[]) {
  const allowed = new Set(keys)
  if (Object.keys(item).some((key) => !allowed.has(key)))
    throw new Error("Conversation response has unknown fields")
}
function text(
  item: Record<string, unknown>,
  key: string,
  max = 32768,
  empty = true,
): string {
  const value = item[key]
  if (
    typeof value !== "string" ||
    utf8.encode(value).byteLength > max ||
    (!empty && !value.length)
  )
    throw new Error(`Invalid conversation ${key}`)
  return value
}
function flag(item: Record<string, unknown>, key: string) {
  if (typeof item[key] !== "boolean") throw new Error(`Invalid conversation ${key}`)
}
function items(
  item: Record<string, unknown>,
  key: string,
  max: number,
): Record<string, unknown>[] {
  const value = item[key]
  if (!Array.isArray(value) || value.length > max)
    throw new Error(`Invalid conversation ${key}`)
  return value.map(record)
}
function identity(item: Record<string, unknown>, key: string) {
  return text(item, key, 256, false)
}
/** Field order is the serializer's business; compare what the images are. */
function imagesKey(images: readonly ImageAttachment[]) {
  return JSON.stringify(images.map((image) => [image.digest, image.mimeType, image.size]))
}
/**
 * The app that wrote a turn (`ConversationMessageApp`), checked against the
 * schema, as a key: absent is the person's own.
 */
function messageAppKey(value: unknown, what: string): string {
  if (value === undefined) return ""
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`Invalid ${what}`)
  const app = value as Record<string, unknown>
  exact(app, ["executionId", "toolId", "server", "tool"])
  identity(app, "executionId")
  identity(app, "toolId")
  if (
    !boundedName(app.server, bounds.maxMcpNameBytes) ||
    !boundedName(app.tool, bounds.maxMcpNameBytes)
  )
    throw new Error(`Invalid ${what} name`)
  return JSON.stringify([app.executionId, app.toolId, app.server, app.tool])
}

/** The same, for the paths a turn points at: the paths, in order. */
function filesKey(files: readonly LinkedFile[]) {
  return JSON.stringify(files.map((file) => file.path))
}
function oneOf(value: string, values: readonly string[]) {
  if (!values.includes(value)) throw new Error("Invalid conversation state")
}

export function conversationId(
  value: unknown,
  expected: string,
): { conversationId: string } {
  const item = record(value)
  exact(item, ["conversationId"])
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  return { conversationId: expected }
}
/** A review's key, as a view lists it and `permissionReview` reads it. */
export const reviewKeyPattern = /^[0-9a-f]{32}$/
/**
 * What one waiting review shows, read whole with `permissionReview`: who
 * asked, what it asks, its whole input, every choice, and any deadline.
 */
export function conversationPermission(value: unknown): ConversationPermission {
  const permission = record(value)
  exact(permission, [
    "review",
    "executionId",
    "permissionId",
    "toolId",
    "title",
    "options",
    "toolName",
    "argumentsJson",
    "origin",
    "ask",
    "deadline",
  ])
  if (!reviewKeyPattern.test(text(permission, "review", 32)))
    throw new Error("Invalid conversation review key")
  for (const key of ["executionId", "permissionId", "toolId"]) identity(permission, key)
  // What it asks the person to allow, a closed set: a surface words its
  // review by it, so a value this build does not know refuses the review.
  const ask = text(permission, "ask", 16)
  oneOf(ask, Object.values(ConversationPermissionAsk))
  // Who asked: the agent, or an MCP App naming the tool it asked to call.
  const origin = record(permission.origin)
  exact(origin, ["kind", "server", "tool"])
  const kind = text(origin, "kind", 16)
  oneOf(kind, ["harness", "app"])
  if (kind === "app") {
    text(origin, "server", bounds.maxMcpNameBytes, false)
    text(origin, "tool", bounds.maxMcpNameBytes, false)
  } else if (origin.server !== undefined || origin.tool !== undefined) {
    throw new Error("A review the agent asked for names no app tool")
  } else if (ask === ConversationPermissionAsk.Message) {
    // Only an app sends a message as the person; the agent asks to run tools.
    throw new Error("A review the agent asked for asks to send a message")
  }
  text(permission, "title", 2048)
  const toolName = text(permission, "toolName", bounds.maxReviewToolNameBytes)
  // The card names the app's tool; the answer approves toolName.
  if (kind === "app" && origin.tool !== toolName)
    throw new Error("An app's review names a tool other than the one it reviews")
  // The agent's own text, whole, shown as it is; it need not be JSON.
  text(permission, "argumentsJson", bounds.maxReviewInputBytes)
  const options = items(permission, "options", bounds.maxReviewOptions)
  if (!options.length) throw new Error("Permission response has no choices")
  const ids = new Set<string>()
  for (const option of options) {
    exact(option, ["id", "label", "effect"])
    const id = identity(option, "id")
    if (ids.has(id)) throw new Error("Permission response repeats an option")
    ids.add(id)
    text(option, "label", bounds.maxReviewOptionLabelBytes, false)
    oneOf(text(option, "effect"), Object.values(ConversationPermissionOptionEffect))
  }
  if (permission.deadline !== undefined) deadlineOf(permission.deadline)
  return permission as unknown as ConversationPermission
}
/**
 * All one waiting question asks, read whole with `questionReview`: the
 * agent's message and every question, with its options. A question is not a
 * review: nothing is authorised, and what comes back is the agent's own
 * input. It is checked as strictly all the same, because the panel renders
 * whatever survives this.
 */
export function conversationQuestion(value: unknown): ConversationQuestion {
  const question = record(value)
  exact(question, ["question", "executionId", "questionId", "message", "questions"])
  if (!reviewKeyPattern.test(text(question, "question", 32)))
    throw new Error("Invalid conversation question key")
  for (const key of ["executionId", "questionId"]) identity(question, key)
  text(question, "message", 1024)
  const asked = items(question, "questions", 16)
  if (!asked.length) throw new Error("Question response asks nothing")
  const keys = new Set<string>()
  for (const one of asked) {
    exact(one, [
      "key",
      "prompt",
      "header",
      "multiSelect",
      "freeText",
      "required",
      "options",
    ])
    const key = identity(one, "key")
    if (keys.has(key)) throw new Error("Question repeats a key")
    keys.add(key)
    text(one, "prompt", 1024)
    if (one.header !== undefined && one.header !== null) text(one, "header", 1024)
    flag(one, "multiSelect")
    flag(one, "freeText")
    flag(one, "required")
    const options = items(one, "options", 32)
    if (!options.length) throw new Error("Question offers no answers")
    const values = new Set<string>()
    for (const option of options) {
      exact(option, ["value", "label", "description"])
      const value = text(option, "value", 1024)
      if (values.has(value)) throw new Error("Question repeats an answer")
      values.add(value)
      text(option, "label", 1024)
      if (option.description !== undefined && option.description !== null)
        text(option, "description", 1024)
    }
  }
  return question as unknown as ConversationQuestion
}
/**
 * One range of what a waiting review or question shows, held to the range asked: its
 * offset, the review's whole length, and canonical base64 of exactly the
 * bytes left up to `maxBytes`. `totalBytes`, when known from an earlier
 * range, must not change between ranges.
 */
export function permissionReviewRange(
  value: unknown,
  offset: number,
  maxBytes: number,
  totalBytes?: number,
): { totalBytes: number; bytes: Uint8Array } {
  const item = record(value)
  exact(item, ["offset", "totalBytes", "data"])
  if (item.offset !== offset) throw new Error("Review response is for another range")
  const total = item.totalBytes
  if (
    !Number.isSafeInteger(total) ||
    (total as number) <= 0 ||
    (total as number) > bounds.maxReviewBytes ||
    (totalBytes !== undefined && total !== totalBytes) ||
    offset >= (total as number)
  )
    throw new Error("Review response names another length")
  const length = Math.min(maxBytes, (total as number) - offset)
  const bytes = canonicalBase64(item.data, maxBytes, true)
  if (bytes.byteLength !== length)
    throw new Error("Review response is not the range asked for")
  return { totalBytes: total as number, bytes }
}
/** Authoritative local activity; unlike a replacement view this cannot truncate the parent. */
export function conversationPeek(
  value: unknown,
  expected: string,
): ConversationPeekResult {
  const item = record(value)
  exact(item, ["conversationId", "executionId"])
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  const executionId = item.executionId === null ? null : text(item, "executionId", 256)
  if (executionId !== null && !executionId.trim())
    throw new Error("Invalid active execution ID")
  return { conversationId: expected, executionId }
}

export function conversationReceipt(
  value: unknown,
  executionId: string,
): ConversationReceipt {
  const item = { ...record(value) }
  exact(item, ["executionId", "disposition"])
  if (identity(item, "executionId") !== executionId)
    throw new Error("Conversation receipt belongs to another execution")
  const disposition = text(item, "disposition")
  oneOf(disposition, ["queued", "injected", "settled"])
  return { executionId, disposition } as ConversationReceipt
}
export function conversationCommandReceipt(
  value: unknown,
  requestId: string,
): { requestId: string; stage: string; outcome?: string } {
  const item = record(value)
  exact(item, ["requestId", "stage", "outcome"])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation receipt belongs to another action")
  oneOf(text(item, "stage"), ["accepted", "attempted", "ready", "settled"])
  if (item.outcome !== undefined)
    oneOf(text(item, "outcome"), [
      "dispatched",
      "withdrawn",
      "cancelled",
      "already_final",
      "no_effect",
      "refused_after_save",
      "unavailable_after_save",
    ])
  return item as { requestId: string; stage: string; outcome?: string }
}
export function conversationMutation(
  value: unknown,
  requestId: string,
): ConversationMutationResult {
  const item = record(value)
  exact(item, ["requestId", "applied"])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation receipt belongs to another action")
  flag(item, "applied")
  return item as unknown as ConversationMutationResult
}
/** Confirm a share's answer belongs to its request and names the grant's revision. */
export function conversationShareResult(
  value: unknown,
  requestId: string,
): ConversationShareResult {
  const item = record(value)
  exact(item, ["requestId", "applied", "policyRevision"])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation receipt belongs to another action")
  flag(item, "applied")
  // A safe integer, so at most `maxPolicyRevision`; never below the first.
  if (time(item, "policyRevision") < minPolicyRevision)
    throw new Error("Invalid conversation policyRevision")
  return item as unknown as ConversationShareResult
}
/**
 * The largest policy revision the schema carries: `expectedPolicyRevision`
 * on a share or revoke and `policyRevision` on a share's answer, a JSON safe
 * integer (tied to the schema in `conversation-validate.test.ts`).
 */
export const maxPolicyRevision = Number.MAX_SAFE_INTEGER
/** The least revision a change may name: 0, which names no grant held. */
export const minExpectedPolicyRevision = 0
/**
 * The least policy revision a share answers or a grant is listed at: a
 * revision is the gateway's journal sequence, which starts at 1, so 0 only
 * ever names "no grant held" in what a change sends.
 */
export const minPolicyRevision = 1

/** Confirm the reply belongs to the requested change and names the exact preset. */
export function conversationApprovalMode(
  value: unknown,
  requestId: string,
  requested: ApprovalMode,
): ConversationSetApprovalModeResult {
  const item = record(value)
  exact(item, ["requestId", "mode"])
  if (identity(item, "requestId") !== requestId || text(item, "mode") !== requested)
    throw new Error("Conversation mode result contradicts the requested change")
  return item as unknown as ConversationSetApprovalModeResult
}

/** Confirm the reply belongs to the requested preference and names that choice. */
export function conversationUsageLimitAutoContinue(
  value: unknown,
  requestId: string,
  requested: boolean,
): ConversationSetUsageLimitAutoContinueResult {
  const item = record(value)
  exact(item, ["requestId", "enabled"])
  if (identity(item, "requestId") !== requestId || item.enabled !== requested)
    throw new Error(
      "Conversation usage-limit preference contradicts the requested change",
    )
  return item as unknown as ConversationSetUsageLimitAutoContinueResult
}
/** Correlate the model reply before a caller can display its selection. */
export function conversationModel(
  value: unknown,
  requestId: string,
  requested: string,
): ConversationSetModelResult {
  const item = record(value)
  exact(item, ["requestId", "model", "approvalMode", "effect"])
  if (identity(item, "requestId") !== requestId || text(item, "model") !== requested)
    throw new Error("Conversation model result contradicts the requested change")
  oneOf(text(item, "approvalMode"), ["ask", "auto", "full"])
  oneOf(text(item, "effect"), ["subsequentResponses", "nextTurn"])
  return item as unknown as ConversationSetModelResult
}
export function conversationView(value: unknown, expected: string): ConversationView {
  const item = record(value)
  exact(item, [
    "conversationId",
    "revision",
    "serverNowMs",
    "messages",
    "pending",
    "permissions",
    "questions",
    "tools",
    "capabilities",
    "lifecycle",
    "truncated",
    "interactionViewError",
    "queueComplete",
    "transcriptState",
    "runtime",
    "lease",
    "artifacts",
    "notes",
    "unreadable",
    "unreadableTruncated",
    "usageLimitAutoContinue",
    "title",
    "approvalMode",
    "approvalModes",
    "approvalModeChange",
    "modelChange",
    "modelChangeEffect",
    "model",
  ])
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  identity(item, "revision")
  // The gateway's time the view was sent at: what each deadline's `dueAtMs`
  // is measured against (`clientDueAtMs`).
  const serverNowMs = item.serverNowMs
  if (
    typeof serverNowMs !== "number" ||
    !Number.isSafeInteger(serverNowMs) ||
    serverNowMs < 1
  )
    throw new Error("Invalid conversation serverNowMs")
  if (
    item.usageLimitAutoContinue !== undefined &&
    typeof item.usageLimitAutoContinue !== "boolean"
  )
    throw new Error("Conversation response has invalid usageLimitAutoContinue")
  optionalText(item, "title", bounds.maxConversationTitleBytes)
  flag(item, "truncated")
  flag(item, "queueComplete")
  oneOf(text(item, "transcriptState", maxTranscriptStateBytes, false), transcriptStates)
  const committedMode = text(item, "approvalMode", 4, false)
  const offeredModes = approvalModeChoices(item.approvalModes)
  if (!offeredModes.some((choice) => choice.id === committedMode))
    throw new Error("Conversation mode is unavailable for its model")
  if (item.approvalModeChange !== undefined) {
    const change = record(item.approvalModeChange)
    exact(change, ["requestId", "requestedMode", "status"])
    identity(change, "requestId")
    const requested = text(change, "requestedMode", 4, false)
    if (!offeredModes.some((choice) => choice.id === requested))
      throw new Error("Conversation requested mode is unavailable")
    oneOf(text(change, "status"), ["changing", "recovery_required"])
  }
  if (item.modelChange !== undefined) {
    const change = record(item.modelChange)
    exact(change, ["requestId", "requestedModel", "status"])
    identity(change, "requestId")
    text(change, "requestedModel", 256, false)
    oneOf(text(change, "status"), ["changing", "recovery_required"])
  }
  if (item.modelChangeEffect !== undefined)
    oneOf(text(item, "modelChangeEffect"), ["subsequentResponses", "nextTurn"])
  text(item, "model", 256, false)
  if (item.interactionViewError !== undefined)
    text(item, "interactionViewError", 2048, false)
  const messages = items(item, "messages", 128)
  const messageIds = new Set<string>()
  const messageStatuses = new Map<string, string>()
  const messageTexts = new Map<string, string>()
  const messageImages = new Map<string, string>()
  const messageFiles = new Map<string, string>()
  const messageApps = new Map<string, string>()
  const toolPartIds = new Set<string>()
  const noticePartIds = new Set<string>()
  for (const message of messages) {
    exact(message, [
      "executionId",
      "userText",
      "attachments",
      "files",
      "app",
      "status",
      "error",
      "steeringTarget",
      "authenticationRequired",
      "requiredWorkFailed",
      "usageLimit",
      "parts",
      "steeringOffset",
    ])
    const executionId = identity(message, "executionId")
    if (messageIds.has(executionId))
      throw new Error("Conversation response repeats a message execution")
    if (
      message.authenticationRequired !== undefined &&
      typeof message.authenticationRequired !== "boolean"
    )
      throw new Error("Conversation response has invalid authenticationRequired")
    if (message.authenticationRequired === true && message.status !== "failed")
      throw new Error("Conversation authentication refusal requires a failed turn")
    if (message.usageLimit !== undefined) {
      const limit = record(message.usageLimit)
      exact(limit, ["provider", "recovery", "resetsAtMs", "autoResuming"])
      oneOf(text(limit, "provider"), ["claude", "codex"])
      const recovery = text(limit, "recovery")
      oneOf(recovery, ["timed_reset", "user_action", "unknown"])
      if (message.status !== "failed")
        throw new Error("Conversation usage limit requires a failed turn")
      if (recovery === "timed_reset") {
        const resetsAtMs = limit.resetsAtMs
        if (
          typeof resetsAtMs !== "number" ||
          !Number.isSafeInteger(resetsAtMs) ||
          resetsAtMs < 1_704_067_200_000 ||
          resetsAtMs > 4_102_444_800_000
        )
          throw new Error("Conversation usage limit has no reset instant")
      } else if (limit.resetsAtMs !== undefined) {
        throw new Error("Conversation usage limit carries a reset it cannot use")
      }
      if (limit.autoResuming !== undefined && typeof limit.autoResuming !== "boolean")
        throw new Error("Conversation usage limit has invalid autoResuming")
    }
    if (
      message.requiredWorkFailed !== undefined &&
      typeof message.requiredWorkFailed !== "boolean"
    )
      throw new Error("Conversation response has invalid requiredWorkFailed")
    if (message.steeringTarget !== undefined) {
      const target = identity(message, "steeringTarget")
      if (target === executionId)
        throw new Error("Conversation steering input targets itself")
      if (!item.truncated && !messageIds.has(target))
        throw new Error("Conversation steering input targets later or missing work")
      if (message.steeringOffset === undefined)
        throw new Error("Injected input has no observation offset")
    }
    if (
      message.steeringOffset !== undefined &&
      (!Number.isSafeInteger(message.steeringOffset) ||
        (message.steeringOffset as number) < 0)
    )
      throw new Error("Invalid steering offset")
    let previousOffset = -1
    for (const part of items(message, "parts", 512)) {
      exact(part, [
        "offset",
        "kind",
        "text",
        "toolId",
        "noticeId",
        "noticeKind",
        "messageId",
      ])
      if (!Number.isSafeInteger(part.offset) || (part.offset as number) <= previousOffset)
        throw new Error("Invalid part order")
      previousOffset = part.offset as number
      const kind = text(part, "kind")
      oneOf(kind, ["text", "thought", "tool", "local_notice"])
      text(part, "text")
      const toolId = text(part, "toolId", 256)
      const noticeId = text(part, "noticeId", 20)
      const noticeKind = part.noticeKind
      if (
        noticeKind !== undefined &&
        noticeKind !== "review" &&
        noticeKind !== "hook" &&
        noticeKind !== "deadline"
      )
        throw new Error("Conversation local notice has no source")
      if (kind === "tool") {
        if (!toolId.length) throw new Error("Conversation tool part has no tool identity")
        const key = JSON.stringify([executionId, toolId])
        // A tool call is one part, however many updates it has (#418).
        if (toolPartIds.has(key))
          throw new Error("Conversation response repeats a tool call part")
        toolPartIds.add(key)
      } else if (toolId.length) {
        throw new Error("Conversation non-tool part has a tool identity")
      }
      if (kind === "local_notice") {
        if (noticeKind !== "review" && noticeKind !== "hook" && noticeKind !== "deadline")
          throw new Error("Conversation local notice has no source")
        if (
          !/^[1-9][0-9]{0,19}$/.test(noticeId) ||
          (noticeId.length === 20 && noticeId > "18446744073709551615")
        )
          throw new Error("Conversation local notice has no valid identity")
        const key = JSON.stringify([executionId, noticeKind, noticeId])
        if (noticePartIds.has(key))
          throw new Error("Conversation response repeats a local notice identity")
        noticePartIds.add(key)
      } else if (noticeId.length || noticeKind !== undefined) {
        throw new Error("Conversation non-notice part has a notice identity")
      }
      if (part.messageId !== undefined) identity(part, "messageId")
    }
    const userText = text(message, "userText")
    const status = text(message, "status")
    oneOf(status, [
      "queued",
      "running",
      "completed",
      "cancelled",
      "failed",
      "injected",
      "unresolved",
    ])
    const hasSteeringTarget = message.steeringTarget !== undefined
    const hasSteeringOffset = message.steeringOffset !== undefined
    if (
      (status === "injected") !== hasSteeringTarget ||
      hasSteeringTarget !== hasSteeringOffset
    )
      throw new Error("Injected conversation state is incomplete")
    if (message.error !== undefined) text(message, "error", 2048, false)
    if (message.requiredWorkFailed === true) {
      const typedRefusal =
        message.authenticationRequired === true || message.usageLimit !== undefined
      if (!typedRefusal || status !== "failed")
        throw new Error(
          "Conversation required-work failure belongs on a failed authentication refusal or usage limit",
        )
      if (message.error === undefined)
        throw new Error("Conversation required-work failure has no diagnostic")
    }
    messageIds.add(executionId)
    messageStatuses.set(executionId, status)
    messageTexts.set(executionId, userText)
    messageImages.set(
      executionId,
      imagesKey(imageAttachments(message.attachments, "message attachments")),
    )
    messageFiles.set(executionId, filesKey(linkedFiles(message.files, "message files")))
    messageApps.set(executionId, messageAppKey(message.app, "message app"))
  }
  const pendingIds = new Set<string>()
  for (const pending of items(item, "pending", 128)) {
    exact(pending, ["executionId", "text", "attachments", "files", "app", "mode"])
    const executionId = identity(pending, "executionId")
    if (pendingIds.has(executionId))
      throw new Error("Conversation response repeats a pending execution")
    const status = messageStatuses.get(executionId)
    if (status !== undefined && status !== "queued")
      throw new Error("Pending execution is not queued")
    if (status === undefined && !item.truncated)
      throw new Error("Pending execution is missing its message")
    pendingIds.add(executionId)
    const pendingText = text(pending, "text", 8192)
    const pendingImages = imagesKey(
      imageAttachments(pending.attachments, "pending attachments"),
    )
    const pendingFiles = filesKey(linkedFiles(pending.files, "pending files"))
    const pendingApp = messageAppKey(pending.app, "pending app")
    oneOf(text(pending, "mode"), ["queued", "steering"])
    // The waiting input and its queued message are one submission seen twice;
    // the same text over different images is as contradictory as different text.
    if (
      !item.truncated &&
      item.queueComplete &&
      (messageTexts.get(executionId) !== pendingText ||
        messageImages.get(executionId) !== pendingImages ||
        messageFiles.get(executionId) !== pendingFiles ||
        messageApps.get(executionId) !== pendingApp)
    )
      throw new Error("Pending execution contradicts its queued message")
  }
  // Each waiting question is listed by a small entry under its key: all it
  // asks is read with `questionReview` and checked there.
  const questionKeys = new Set<string>()
  for (const entry of items(item, "questions", bounds.maxOpenQuestions)) {
    exact(entry, ["question", "message"])
    const key = text(entry, "question", 32)
    if (!reviewKeyPattern.test(key)) throw new Error("Invalid conversation question key")
    if (questionKeys.has(key)) throw new Error("Conversation response repeats a question")
    questionKeys.add(key)
    text(entry, "message", bounds.maxQuestionEntryMessageBytes)
  }
  // Each waiting review is listed by its key alone, small and of one size:
  // what it shows is read with `permissionReview` and checked there.
  const reviewKeys = new Set<string>()
  for (const entry of items(item, "permissions", bounds.maxListedReviews)) {
    exact(entry, ["review", "origin", "toolName"])
    const key = text(entry, "review", 32)
    if (!reviewKeyPattern.test(key)) throw new Error("Invalid conversation review key")
    if (reviewKeys.has(key)) throw new Error("Conversation response repeats a review")
    reviewKeys.add(key)
    oneOf(text(entry, "origin", 16), ["harness", "app"])
    text(entry, "toolName", bounds.maxReviewEntryToolNameBytes)
  }
  const toolIds = new Set<string>()
  // One execution runs in one process, so its tools share that start's choice.
  const executionHooks = new Map<string, string>()
  for (const tool of items(item, "tools", 128)) {
    exact(tool, [
      "executionId",
      "toolId",
      "title",
      "kind",
      "status",
      "details",
      "input",
      "mcp",
      "structuredContent",
      "nativeHooks",
    ])
    // Absent when the process start the call ran under recorded no choice.
    if (Object.hasOwn(tool, "nativeHooks")) {
      const nativeHooks = text(tool, "nativeHooks", 3)
      if (nativeHooks !== "off" && nativeHooks !== "on")
        throw new Error("Invalid native hooks choice")
    }
    const executionId = identity(tool, "executionId")
    if (Object.hasOwn(tool, "nativeHooks")) {
      const recorded = executionHooks.get(executionId)
      if (recorded !== undefined && recorded !== tool.nativeHooks)
        throw new Error("An execution names two native hooks choices")
      executionHooks.set(executionId, tool.nativeHooks as string)
    }
    identity(tool, "toolId")
    const toolKey = JSON.stringify([tool.executionId, tool.toolId])
    if (toolIds.has(toolKey)) throw new Error("Conversation response repeats a tool")
    toolIds.add(toolKey)
    text(tool, "title", 2048)
    // The provider's own category, empty until it says. The schema names the
    // values, so an unknown one is a gateway this client does not understand.
    oneOf(text(tool, "kind", 32), [
      "",
      "read",
      "edit",
      "search",
      "fetch",
      "execute",
      "think",
      "delete",
      "move",
      "switch_mode",
      "other",
    ])
    text(tool, "status", 64)
    text(tool, "details", 16384)
    text(tool, "input", 32768)
    // Present only once the harness named the MCP server and tool; which names
    // are valid is the SDK's rule, and only the published bound is checked here.
    if (tool.mcp !== undefined) {
      const mcp = record(tool.mcp)
      exact(mcp, ["server", "tool", "resourceUri", "argumentsJson"])
      text(mcp, "server", bounds.maxMcpNameBytes, false)
      text(mcp, "tool", bounds.maxMcpNameBytes, false)
      // The UI the gateway read from the server itself; which URIs are valid is
      // the SDK's rule, and only the published bound is checked here.
      if (mcp.resourceUri !== undefined)
        text(mcp, "resourceUri", bounds.maxUiResourceUriBytes, false)
      // The arguments the connection saw, within the same published bound as
      // a review and an app's own call. Absent when it could not match them.
      if (mcp.argumentsJson !== undefined)
        text(mcp, "argumentsJson", bounds.maxMcpArgumentsBytes)
    }
    if (tool.structuredContent !== undefined)
      text(tool, "structuredContent", bounds.maxToolStructuredContentBytes)
  }
  if (!item.truncated) {
    for (const toolKey of toolIds) {
      if (!toolPartIds.has(toolKey))
        throw new Error("Conversation tool is orphaned from its message")
    }
    for (const toolKey of toolPartIds) {
      if (!toolIds.has(toolKey))
        throw new Error("Conversation tool part has no matching tool state")
    }
  }
  if (item.runtime !== undefined) {
    const runtime = record(item.runtime)
    exact(runtime, [
      "model",
      "provider",
      "workspace",
      "agent",
      "modelName",
      "contextWindowTokens",
      "reasoning",
    ])
    for (const key of ["model", "provider", "workspace"]) text(runtime, key, 4096)
    text(runtime, "agent", 32, false)
    text(runtime, "modelName", 256, false)
    if (
      !Number.isSafeInteger(runtime.contextWindowTokens) ||
      (runtime.contextWindowTokens as number) < 1 ||
      (runtime.contextWindowTokens as number) > 4294967295
    )
      throw new Error("Invalid conversation model context window")
    flag(runtime, "reasoning")
  }
  if (item.lease !== undefined) lease(item.lease)
  for (const artifact of items(item, "artifacts", 64)) conversationArtifact(artifact)
  for (const note of items(item, "notes", 64)) conversationNote(note)
  const unreadIds = new Set<string>()
  for (const part of items(item, "unreadable", 32)) {
    const id = conversationUnreadable(part)
    if (unreadIds.has(id)) throw new Error("Repeated conversation unreadable id")
    unreadIds.add(id)
  }
  flag(item, "unreadableTruncated")
  const capabilities = record(item.capabilities)
  const capabilityKeys = [
    "queue",
    "steer",
    "resume",
    "permissions",
    "imageInput",
    "agentFeatures",
  ]
  exact(capabilities, capabilityKeys)
  for (const key of capabilityKeys.slice(0, 5)) flag(capabilities, key)
  const confirmed =
    item.transcriptState === "complete" || item.transcriptState === "complete_empty"
  if (
    !confirmed &&
    (item.queueComplete ||
      capabilities.queue ||
      capabilities.steer ||
      capabilities.permissions ||
      pendingIds.size)
  )
    throw new Error("Unconfirmed conversation history offers controls")
  if (
    item.transcriptState === "complete_empty" &&
    (messages.length || pendingIds.size || toolIds.size)
  )
    throw new Error("Empty conversation history contains transcript evidence")
  const features = record(capabilities.agentFeatures)
  const featureValues = {
    permissionDenial: PermissionDenialSupport,
    nativeHookSuppression: NativeHookSuppressionSupport,
    nativeHooks: NativeHooksChoice,
    compactionReporting: CompactionReportingSupport,
    modelSwitchReporting: ModelSwitchReportingSupport,
    permissionDeferral: PermissionDeferralSupport,
    elicitationForwarding: ElicitationForwardingSupport,
    preToolPolicy: PreToolPolicySupport,
    policyEndTurn: PolicyEndTurnSupport,
    policyCloseSession: PolicyCloseSessionSupport,
    incomingElicitation: IncomingElicitationSupport,
  } as const
  exact(features, Object.keys(featureValues))
  for (const [key, values] of Object.entries(featureValues))
    oneOf(text(features, key), Object.values(values))
  const lifecycle = record(item.lifecycle)
  exact(lifecycle, ["phase", "failure", "evidenceFailure"])
  const phase = text(lifecycle, "phase")
  oneOf(phase, ["absent", "starting", "attached", "failed"])
  const attachmentFailure = (value: unknown) => {
    const failure = record(value)
    exact(failure, ["code", "message"])
    oneOf(text(failure, "code"), ["audit", "provider", "storage", "cleanup"])
    text(failure, "message", 2048, false)
  }
  if (phase === "failed") {
    attachmentFailure(lifecycle.failure)
  } else if (lifecycle.failure !== undefined) {
    throw new Error("Conversation lifecycle failure contradicts its phase")
  }
  if (lifecycle.evidenceFailure !== undefined)
    attachmentFailure(lifecycle.evidenceFailure)
  if (
    lifecycle.evidenceFailure !== undefined &&
    record(lifecycle.evidenceFailure).code !== "audit"
  )
    throw new Error("Conversation lifecycle evidence has an invalid cause")
  return item as unknown as ConversationView
}

// Each set is checked against the generated type both ways: a value the schema
// gains or loses fails to compile here rather than being refused on the wire.
const leaseValues = {
  state: {
    live: true,
    ending: true,
    ended: true,
    interrupted: true,
    refused: true,
    unreadable: true,
  } satisfies Record<ConversationLease["state"], true>,
  environment: { here: true, ssh: true, peer: true } satisfies Record<
    NonNullable<ConversationLease["environment"]>,
    true
  >,
  sandbox: { harness_default: true, none: true } satisfies Record<
    NonNullable<ConversationLease["sandbox"]>,
    true
  >,
  cause: {
    stopped: true,
    closed: true,
    revoked: true,
    expired: true,
    lost: true,
    idle: true,
  } satisfies Record<NonNullable<ConversationLease["cause"]>, true>,
  cleanup: { confirmed: true, forced: true, not_held: true } satisfies Record<
    NonNullable<ConversationLease["cleanup"]>,
    true
  >,
  refusal: {
    sandbox_unavailable: true,
    environment_unreachable: true,
    environment_version_mismatch: true,
    environment_busy: true,
    agent_unavailable: true,
    environment_platform_unsupported: true,
    environment_install_failed: true,
  } satisfies Record<NonNullable<ConversationLease["refusal"]>, true>,
}

type LeaseField = "required" | "optional" | "absent"
type LeaseShape = Record<
  "revision" | "environment" | "sandbox" | "cause" | "cleanup" | "refusal" | "commands",
  LeaseField
> & {
  /** Whether events can have been dropped: only once it stopped accepting them. */
  dropped: boolean
}

const issued = {
  revision: "required",
  environment: "required",
  sandbox: "required",
  refusal: "absent",
  commands: "absent",
} as const

/** Which fields each state carries: exactly what the gateway's `lease_view`
 * can produce. A lease is issued with its terms and revision; a cause once it
 * is ending, a cleanup once that is reported (late, for an interrupted one);
 * events are dropped only after it stopped accepting them; commands are
 * counted only while it can still have them running. A refused lease
 * has its terms and refusal and nothing after; an unreadable one claims
 * nothing at all. */
const leaseShapes = {
  live: {
    ...issued,
    cause: "absent",
    cleanup: "absent",
    commands: "optional",
    dropped: false,
  },
  ending: {
    ...issued,
    cause: "required",
    cleanup: "absent",
    commands: "optional",
    dropped: false,
  },
  ended: { ...issued, cause: "required", cleanup: "required", dropped: true },
  interrupted: {
    ...issued,
    cause: "required",
    cleanup: "optional",
    dropped: true,
  },
  refused: {
    ...issued,
    refusal: "required",
    cause: "absent",
    cleanup: "absent",
    dropped: false,
  },
  unreadable: {
    revision: "absent",
    environment: "absent",
    sandbox: "absent",
    cause: "absent",
    cleanup: "absent",
    refusal: "absent",
    commands: "absent",
    dropped: false,
  },
} satisfies Record<ConversationLease["state"], LeaseShape>

/** The view's lease: the published shape, and only the fields its state
 * carries, so a lease that contradicts itself is refused rather than shown. */
function lease(value: unknown) {
  const item = record(value)
  exact(item, [
    "state",
    "revision",
    "environment",
    "host",
    "sandbox",
    "cause",
    "cleanup",
    "refusal",
    "droppedEvents",
    "commands",
  ])
  for (const [key, values] of Object.entries(leaseValues)) {
    const field = item[key]
    if (key !== "state" && field === undefined) continue
    if (typeof field !== "string" || !Object.hasOwn(values, field))
      throw new Error(`Invalid conversation lease ${key}`)
  }
  const count = (key: string, min: number) => {
    const field = item[key]
    if (!Number.isSafeInteger(field) || (field as number) < min)
      throw new Error(`Invalid conversation lease ${key}`)
  }
  if (item.revision !== undefined) count("revision", 1)
  count("droppedEvents", 0)
  // At most the four commands a lease lets run at once.
  if (item.commands !== undefined) {
    count("commands", 1)
    if ((item.commands as number) > 4)
      throw new Error("Invalid conversation lease commands")
  }
  const shape: LeaseShape = leaseShapes[item.state as ConversationLease["state"]]
  for (const key of [
    "revision",
    "environment",
    "sandbox",
    "cause",
    "cleanup",
    "refusal",
    "commands",
  ] as const) {
    const present = item[key] !== undefined
    if ((shape[key] === "required" && !present) || (shape[key] === "absent" && present))
      throw new Error(`Invalid conversation lease ${key}`)
  }
  if (!shape.dropped && item.droppedEvents !== 0)
    throw new Error("Invalid conversation lease droppedEvents")
  // The machine's name, exactly when the lease names a machine of its own:
  // an SSH destination, or a paired environment's `peer:` and its key, 32
  // bytes in lowercase hex, as the gateway writes it.
  if (item.environment === "ssh") text(item, "host", 253, false)
  else if (item.environment === "peer") {
    if (typeof item.host !== "string" || !PEER_ENVIRONMENT.test(item.host))
      throw new Error("Invalid conversation lease host")
  } else if (item.host !== undefined) throw new Error("Invalid conversation lease host")
}

/** A paired environment's name: `peer:` and its 32-byte key in lowercase hex. */
const PEER_ENVIRONMENT = /^peer:[0-9a-f]{64}$/

/** One artifact the view lists: the published shape, closed. The conversation
 * keeps an artifact as one of its uploads, so it is bounded as an upload is. */
function conversationArtifact(item: Record<string, unknown>) {
  exact(item, ["executionId", "lease", "name", "digest", "mimeType", "size"])
  if (item.executionId !== undefined) identity(item, "executionId")
  text(item, "lease", 128, false)
  text(item, "name", 255, false)
  if (!validDigest(item.digest)) throw new Error("Invalid conversation artifact digest")
  if (!validMediaType(item.mimeType))
    throw new Error("Invalid conversation artifact mimeType")
  if (!validSize(item.size, MAX_UPLOAD_BYTES))
    throw new Error("Invalid conversation artifact size")
}

/** Where a note stands, with the turn that took it and why it was retired
 * present exactly when its state says so. */
function noteState(item: Record<string, unknown>) {
  const state = text(item, "deliveryState")
  oneOf(state, ["recorded", "pending", "assigned", "retired"])
  if ((state === "assigned") !== (item.executionId !== undefined))
    throw new Error("Invalid conversation note executionId")
  if (item.executionId !== undefined) identity(item, "executionId")
  if ((state === "retired") !== (item.retiredReason !== undefined))
    throw new Error("Invalid conversation note retiredReason")
  if (item.retiredReason !== undefined)
    oneOf(text(item, "retiredReason"), ["expired", "permission_lost"])
}
/** A note's text and its acceptance time, as its receipt and the view state them. */
function noteAccepted(item: Record<string, unknown>) {
  text(item, "messageId", 64, false)
  const acceptedAtMs = item.acceptedAtMs
  if (
    typeof acceptedAtMs !== "number" ||
    !Number.isSafeInteger(acceptedAtMs) ||
    acceptedAtMs < 0
  )
    throw new Error("Invalid conversation acceptedAtMs")
}

/** One record the view could not read: its position, and the turn it follows. */
function conversationUnreadable(item: Record<string, unknown>): string {
  exact(item, ["id", "afterExecutionId"])
  const id = text(item, "id", 20, false)
  if (!/^(0|[1-9][0-9]*)$/.test(id)) throw new Error("Invalid conversation unreadable id")
  if (item.afterExecutionId !== undefined) identity(item, "afterExecutionId")
  return id
}

/** One note the view lists: the published shape, closed. */
function conversationNote(item: Record<string, unknown>) {
  exact(item, [
    "messageId",
    "text",
    "acceptedAtMs",
    "delivery",
    "deliveryState",
    "executionId",
    "retiredReason",
    "afterExecutionId",
    "author",
  ])
  noteAccepted(item)
  text(item, "text", 8192, false)
  oneOf(text(item, "delivery"), ["record_only", "next_turn"])
  noteState(item)
  if (item.afterExecutionId !== undefined) identity(item, "afterExecutionId")
  const author = record(item.author)
  exact(author, ["kind", "surface"])
  oneOf(text(author, "kind"), ["owner", "grant"])
  text(author, "surface", 256, false)
}

/** Confirm a note's receipt answers the request that left it. */
export function conversationMessageReceipt(
  value: unknown,
  requestId: string,
): ConversationMessageReceipt {
  const item = record(value)
  exact(item, [
    "requestId",
    "messageId",
    "acceptedAtMs",
    "deliveryState",
    "executionId",
    "retiredReason",
  ])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation receipt belongs to another action")
  noteAccepted(item)
  noteState(item)
  return item as unknown as ConversationMessageReceipt
}

/** Confirm a note's status answers the request looked up: a found note
 * carries its name, time and state; one not found carries none of them. */
export function conversationMessageStatus(
  value: unknown,
  requestId: string,
): ConversationMessageStatusResult {
  const item = record(value)
  exact(item, [
    "found",
    "requestId",
    "messageId",
    "acceptedAtMs",
    "deliveryState",
    "executionId",
    "retiredReason",
  ])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation status belongs to another action")
  flag(item, "found")
  if (item.found) {
    noteAccepted(item)
    noteState(item)
  } else if (
    ["messageId", "acceptedAtMs", "deliveryState", "executionId", "retiredReason"].some(
      (key) => item[key] !== undefined,
    )
  )
    throw new Error("Conversation status describes a note it did not find")
  return item as unknown as ConversationMessageStatusResult
}

export function conversationReorder(
  value: unknown,
  requestId: string,
): ConversationReorderResult {
  const item = record(value)
  exact(item, ["requestId", "outcome"])
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation reorder belongs to another action")
  oneOf(text(item, "outcome"), [
    "applied",
    "unchanged",
    "queue_changed",
    "priority_conflict",
  ])
  return item as unknown as ConversationReorderResult
}

/** Null, or text of 1 to `max` UTF-8 bytes: what a summary has not got is null, never "". */
function optionalText(item: Record<string, unknown>, key: string, max: number) {
  return item[key] === null ? null : text(item, key, max, false)
}
function time(item: Record<string, unknown>, key: string) {
  const value = item[key]
  if (!Number.isSafeInteger(value) || (value as number) < 0)
    throw new Error(`Invalid conversation ${key}`)
  return value as number
}

/** One summary row, for a list and for an observation page. */
function summaryRow(
  row: Record<string, unknown>,
  archived: boolean,
  seen: Set<string>,
  filter: string,
): ConversationSummary {
  exact(row, [
    "conversationId",
    "title",
    "preview",
    "createdAtMs",
    "updatedAtMs",
    "running",
    "archived",
  ])
  const conversationId = identity(row, "conversationId")
  if (!conversationIdPattern.test(conversationId) || seen.has(conversationId))
    throw new Error("Invalid conversation conversationId")
  seen.add(conversationId)
  flag(row, "running")
  flag(row, "archived")
  if (row.archived !== archived) throw new Error(filter)
  return {
    conversationId,
    title: optionalText(row, "title", bounds.maxConversationTitleBytes),
    preview: optionalText(row, "preview", bounds.maxConversationPreviewBytes),
    createdAtMs: time(row, "createdAtMs"),
    updatedAtMs: time(row, "updatedAtMs"),
    running: row.running as boolean,
    archived: row.archived as boolean,
  }
}

/**
 * The caller's conversations as the gateway listed them. Each row is checked
 * against the schema's bounds, and a conversation listed twice is refused: the
 * list is keyed by identity, and a second row for one conversation would be
 * two answers to what it is called. Every row must also be what was asked for
 * — archived or not — since a row on the wrong side of the filter is a list
 * that contradicts its own request.
 */
export function conversationList(
  value: unknown,
  archived: boolean,
): ConversationListResult {
  const item = record(value)
  exact(item, ["conversations", "complete"])
  flag(item, "complete")
  const seen = new Set<string>()
  const conversations = items(item, "conversations", bounds.maxListedConversations).map(
    (row) =>
      summaryRow(
        row,
        archived,
        seen,
        "Conversation list row contradicts the archived filter",
      ),
  )
  return { conversations, complete: item.complete as boolean }
}

/**
 * One page of `conversation.observe`. The row checks are the list's. A page
 * longer than the catalogue bound is refused. `complete: false` is kept,
 * including when the page has no cursor: that page is not every stored summary.
 */
export function conversationObserve(
  value: unknown,
  archived: boolean,
): ConversationObserveResult {
  const item = record(value)
  exact(item, ["conversations", "complete", "cursor"])
  flag(item, "complete")
  const seen = new Set<string>()
  const conversations = items(item, "conversations", bounds.maxCataloguePageEntries).map(
    (row) =>
      summaryRow(
        row,
        archived,
        seen,
        "Conversation observe row contradicts the archived filter",
      ),
  )
  if (!Object.hasOwn(item, "cursor"))
    return { conversations, complete: item.complete as boolean }
  const cursor = record(item.cursor)
  exact(cursor, ["incarnation", "boundary", "creation", "id"])
  const incarnation = text(cursor, "incarnation", bounds.maxSyncIdBytes, false)
  if (!decimal(cursor.boundary) || !decimal(cursor.creation))
    throw new Error("Invalid conversation observe cursor")
  const id = identity(cursor, "id")
  if (!conversationIdPattern.test(id))
    throw new Error("Invalid conversation observe cursor")
  return {
    conversations,
    complete: item.complete as boolean,
    cursor: {
      incarnation,
      boundary: cursor.boundary,
      creation: cursor.creation,
      id,
    },
  }
}

/**
 * A Drive grant's tool policy bounds, as the product schema publishes them:
 * each of allow and deny holds at most `patterns` patterns of at most
 * `patternBytes` UTF-8 bytes, and `commandHosts` holds at most `hosts` hosts
 * of at most `hostBytes` bytes, apart from the patterns. The gateway refuses
 * past them (`invalid_request`); a surface budgets its choices against these
 * rather than numbers of its own.
 */
export const toolPolicyBounds = {
  patterns: bounds.maxToolPolicyPatterns,
  patternBytes: bounds.maxToolPatternBytes,
  hosts: bounds.maxCommandHosts,
  hostBytes: bounds.maxCommandHostBytes,
} as const

/**
 * How long a view's list entries may be, as the product schema publishes
 * them: a review entry's tool name holds at most `toolNameBytes` UTF-8 bytes,
 * and a question entry's message at most `messageBytes`. The gateway clips
 * each to fit; a view past them is refused.
 */
export const viewEntryBounds = {
  toolNameBytes: bounds.maxReviewEntryToolNameBytes,
  messageBytes: bounds.maxQuestionEntryMessageBytes,
} as const

/**
 * How `conversation.shares` is read: one answer holds at most `pageSize`
 * grants, and a conversation holds at most `grants`. A later page echoes the
 * cursor the previous page returned.
 */
export const sharePage = {
  pageSize: bounds.maxSharePage,
  grants: bounds.maxShareGrants,
} as const

/** One list of a tool policy's names: at most `max`, each 1 to `bytes` UTF-8 bytes, each once. */
function policyNames(
  item: Record<string, unknown>,
  key: string,
  max: number,
  bytes: number,
) {
  const value = item[key]
  if (!Array.isArray(value) || value.length > max)
    throw new Error(`Invalid conversation share ${key}`)
  for (const name of value) {
    if (typeof name !== "string" || !name || utf8.encode(name).byteLength > bytes)
      throw new Error(`Invalid conversation share ${key}`)
  }
  return value as string[]
}

/** A Drive grant's tool policy as `conversation.shares` lists it, within the published bounds. */
function toolPolicy(value: unknown) {
  const policy = record(value)
  exact(policy, ["preset", "allow", "deny", "commandHosts"])
  if (policy.preset !== undefined) {
    if (policy.allow !== undefined || policy.deny !== undefined)
      throw new Error("Invalid conversation share policy")
    oneOf(text(policy, "preset", 32), Object.values(ConversationToolPreset))
  } else {
    const patterns =
      policyNames(
        policy,
        "allow",
        toolPolicyBounds.patterns,
        toolPolicyBounds.patternBytes,
      ).length +
      policyNames(
        policy,
        "deny",
        toolPolicyBounds.patterns,
        toolPolicyBounds.patternBytes,
      ).length
    if (patterns === 0) throw new Error("Invalid conversation share policy")
  }
  if (policy.commandHosts !== undefined) {
    const hosts = policyNames(
      policy,
      "commandHosts",
      toolPolicyBounds.hosts,
      toolPolicyBounds.hostBytes,
    )
    // Listed only when it names a host, each once. Hosts are apart from the
    // patterns: a full allow and deny list may still name every host.
    if (!hosts.length || new Set(hosts).size !== hosts.length)
      throw new Error("Invalid conversation share commandHosts")
  }
}

/** The three cumulative capability sets, in any order, each name once. */
function capabilitySet(item: Record<string, unknown>): ConversationCapability[] {
  const value = item.capabilities
  if (!Array.isArray(value)) throw new Error("Invalid conversation capabilities")
  const names = value.map((name) => {
    if (typeof name !== "string") throw new Error("Invalid conversation capabilities")
    oneOf(name, Object.values(ConversationCapability))
    return name
  })
  if (new Set(names).size !== names.length)
    throw new Error("Invalid conversation capabilities")
  const held = new Set(names)
  const legal =
    (held.size === 1 && held.has(ConversationCapability.Read)) ||
    (held.size === 2 &&
      held.has(ConversationCapability.Read) &&
      held.has(ConversationCapability.Comment)) ||
    (held.size === 3 &&
      held.has(ConversationCapability.Read) &&
      held.has(ConversationCapability.Comment) &&
      held.has(ConversationCapability.Drive))
  if (!legal) throw new Error("Invalid conversation capabilities")
  return names as ConversationCapability[]
}

/**
 * Validate one page of `conversation.shares`: each grant once, a policy
 * exactly when the capability set includes drive, the catalogue revision the
 * page was read at, and a cursor only when it names this page's last grant.
 * A page holds at most the published page size.
 */
export function conversationShares(value: unknown): ConversationSharesResult {
  const result = record(value)
  exact(result, ["items", "cursor", "changeRevision"])
  // Read with the grants. A later page belongs to the same read only when it
  // names the same revision.
  time(result, "changeRevision")
  const seen = new Set<string>()
  let last: { credentialId: string; grantedAtMs: number } | undefined
  for (const item of items(result, "items", sharePage.pageSize)) {
    exact(item, [
      "credentialId",
      "capabilities",
      "policy",
      "policyRevision",
      "grantedAtMs",
    ])
    const credential = identity(item, "credentialId")
    if (seen.has(credential)) throw new Error("Repeated conversation share")
    seen.add(credential)
    const granted = capabilitySet(item)
    if (granted.includes(ConversationCapability.Drive) !== (item.policy !== undefined))
      throw new Error("Invalid conversation share policy")
    if (item.policy !== undefined) toolPolicy(item.policy)
    // A listed grant holds a revision the journal gave, never 0 (no grant held).
    if (time(item, "policyRevision") < minPolicyRevision)
      throw new Error("Invalid conversation share policy revision")
    last = { credentialId: credential, grantedAtMs: time(item, "grantedAtMs") }
  }
  if (Object.hasOwn(result, "cursor")) {
    if (last === undefined) throw new Error("Invalid conversation shares cursor")
    const cursor = record(result.cursor)
    exact(cursor, ["grantedAtMs", "credentialId"])
    if (
      identity(cursor, "credentialId") !== last.credentialId ||
      time(cursor, "grantedAtMs") !== last.grantedAtMs
    )
      throw new Error("Invalid conversation shares cursor")
  }
  return result as unknown as ConversationSharesResult
}
