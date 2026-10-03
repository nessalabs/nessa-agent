import type {
  ConversationListResult,
  ConversationSummary,
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationReorderResult,
  ConversationSetApprovalModeResult,
  ApprovalMode,
  ImageAttachment,
  LinkedFile,
} from "../generated/product.js"
import {
  bounds,
  CompactionReportingSupport,
  ConversationPermissionOptionEffect,
  ElicitationForwardingSupport,
  IncomingElicitationSupport,
  ModelSwitchReportingSupport,
  NativeHookSuppressionSupport,
  PermissionDeferralSupport,
  PermissionDenialSupport,
  PolicyCloseSessionSupport,
  PolicyEndTurnSupport,
  PreToolPolicySupport,
} from "../generated/product.js"
import { imageAttachments, linkedFiles } from "./attachment-validate.js"
import { approvalModeChoices } from "./agents-validate.js"
import { boundedName } from "./mcp-app-validate.js"

const utf8 = new TextEncoder()

/** A canonical lowercase hyphenated UUID: the schema's pattern for a conversation identity. */
export const conversationIdPattern = new RegExp(bounds.conversationIdPattern)

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
function oneOf(value: string, values: string[]) {
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
export function conversationReceipt(
  value: unknown,
  executionId: string,
): ConversationReceipt {
  const item = record(value)
  exact(item, ["executionId", "disposition"])
  if (identity(item, "executionId") !== executionId)
    throw new Error("Conversation receipt belongs to another execution")
  oneOf(text(item, "disposition"), ["queued", "injected", "settled"])
  return item as unknown as ConversationReceipt
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
export function conversationView(value: unknown, expected: string): ConversationView {
  const item = record(value)
  exact(item, [
    "conversationId",
    "revision",
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
    "title",
    "approvalMode",
    "approvalModes",
    "approvalModeChange",
  ])
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  identity(item, "revision")
  optionalText(item, "title", bounds.maxConversationTitleBytes)
  flag(item, "truncated")
  flag(item, "queueComplete")
  oneOf(text(item, "transcriptState", 14, false), [
    "not_loaded",
    "partial",
    "complete_empty",
    "complete",
    "stale",
    "unknown",
  ])
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
      "parts",
      "steeringOffset",
    ])
    const executionId = identity(message, "executionId")
    if (messageIds.has(executionId))
      throw new Error("Conversation response repeats a message execution")
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
      exact(part, ["offset", "kind", "text", "toolId", "noticeId", "messageId"])
      if (!Number.isSafeInteger(part.offset) || (part.offset as number) <= previousOffset)
        throw new Error("Invalid part order")
      previousOffset = part.offset as number
      const kind = text(part, "kind")
      oneOf(kind, ["text", "thought", "tool", "local_notice"])
      text(part, "text")
      const toolId = text(part, "toolId", 256)
      const noticeId = text(part, "noticeId", 20)
      if (kind === "tool") {
        if (!toolId.length) throw new Error("Conversation tool part has no tool identity")
        toolPartIds.add(JSON.stringify([executionId, toolId]))
      } else if (toolId.length) {
        throw new Error("Conversation non-tool part has a tool identity")
      }
      if (kind === "local_notice") {
        if (
          !/^[1-9][0-9]{0,19}$/.test(noticeId) ||
          (noticeId.length === 20 && noticeId > "18446744073709551615")
        )
          throw new Error("Conversation local notice has no valid identity")
        const key = JSON.stringify([executionId, noticeId])
        if (noticePartIds.has(key))
          throw new Error("Conversation response repeats a local notice identity")
        noticePartIds.add(key)
      } else if (noticeId.length) {
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
  // A question is not a review: nothing is authorised, and what comes back is
  // the agent's own input. It is checked as strictly all the same, because the
  // panel renders whatever survives this.
  const questionIds = new Set<string>()
  for (const question of items(item, "questions", 8)) {
    exact(question, ["executionId", "questionId", "message", "questions"])
    for (const key of ["executionId", "questionId"]) identity(question, key)
    const questionKey = JSON.stringify([question.executionId, question.questionId])
    if (questionIds.has(questionKey))
      throw new Error("Conversation response repeats a question")
    questionIds.add(questionKey)
    const status = messageStatuses.get(question.executionId as string)
    if (status !== undefined && status !== "running")
      throw new Error("Question execution is not running")
    if (status === undefined && !item.truncated)
      throw new Error("Question execution is missing its message")
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
  }
  const permissionIds = new Set<string>()
  for (const permission of items(item, "permissions", 64)) {
    exact(permission, [
      "executionId",
      "permissionId",
      "toolId",
      "title",
      "options",
      "toolName",
      "argumentsJson",
      "origin",
    ])
    for (const key of ["executionId", "permissionId", "toolId"]) identity(permission, key)
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
    }
    const permissionKey = JSON.stringify([
      permission.executionId,
      permission.permissionId,
    ])
    if (permissionIds.has(permissionKey))
      throw new Error("Conversation response repeats a permission")
    const status = messageStatuses.get(permission.executionId as string)
    // The agent asks while its execution runs. An app asks whenever it is
    // shown, naming its own tool call, which has normally finished.
    if (kind === "harness" && status !== undefined && status !== "running")
      throw new Error("Permission execution is not running")
    if (status === undefined && !item.truncated)
      throw new Error("Permission execution is missing its message")
    permissionIds.add(permissionKey)
    text(permission, "title", 2048)
    text(permission, "toolName", 256)
    JSON.parse(text(permission, "argumentsJson", 32768))
    const options = items(permission, "options", 64)
    if (!options.length) throw new Error("Permission response has no choices")
    const ids = new Set<string>()
    for (const option of options) {
      exact(option, ["id", "label", "effect"])
      const id = identity(option, "id")
      if (ids.has(id)) throw new Error("Permission response repeats an option")
      ids.add(id)
      text(option, "label", 2048, false)
      oneOf(text(option, "effect"), Object.values(ConversationPermissionOptionEffect))
    }
  }
  const toolIds = new Set<string>()
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
    ])
    identity(tool, "executionId")
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
      exact(mcp, ["server", "tool", "resourceUri"])
      text(mcp, "server", bounds.maxMcpNameBytes, false)
      text(mcp, "tool", bounds.maxMcpNameBytes, false)
      // The UI the gateway read from the server itself; which URIs are valid is
      // the SDK's rule, and only the published bound is checked here.
      if (mcp.resourceUri !== undefined)
        text(mcp, "resourceUri", bounds.maxUiResourceUriBytes, false)
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
      pendingIds.size ||
      permissionIds.size ||
      questionIds.size)
  )
    throw new Error("Unconfirmed conversation history offers controls")
  if (
    item.transcriptState === "complete_empty" &&
    (messages.length ||
      pendingIds.size ||
      permissionIds.size ||
      questionIds.size ||
      toolIds.size)
  )
    throw new Error("Empty conversation history contains transcript evidence")
  const features = record(capabilities.agentFeatures)
  const featureValues = {
    permissionDenial: PermissionDenialSupport,
    nativeHookSuppression: NativeHookSuppressionSupport,
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
    (row): ConversationSummary => {
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
      if (row.archived !== archived)
        throw new Error("Conversation list row contradicts the archived filter")
      return {
        conversationId,
        title: optionalText(row, "title", bounds.maxConversationTitleBytes),
        preview: optionalText(row, "preview", bounds.maxConversationPreviewBytes),
        createdAtMs: time(row, "createdAtMs"),
        updatedAtMs: time(row, "updatedAtMs"),
        running: row.running as boolean,
        archived: row.archived as boolean,
      }
    },
  )
  return { conversations, complete: item.complete as boolean }
}
