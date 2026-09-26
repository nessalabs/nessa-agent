import type {
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationReorderResult,
  ImageAttachment,
  LinkedFile,
} from "../generated/product.js"
import { imageAttachments, linkedFiles } from "./attachment-validate.js"

const utf8 = new TextEncoder()

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
    "truncated",
    "permissionViewError",
    "queueComplete",
    "runtime",
  ])
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  identity(item, "revision")
  flag(item, "truncated")
  flag(item, "queueComplete")
  if (item.permissionViewError !== undefined)
    text(item, "permissionViewError", 2048, false)
  const messages = items(item, "messages", 128)
  const messageIds = new Set<string>()
  const messageStatuses = new Map<string, string>()
  const messageTexts = new Map<string, string>()
  const messageImages = new Map<string, string>()
  const messageFiles = new Map<string, string>()
  const toolPartIds = new Set<string>()
  for (const message of messages) {
    exact(message, [
      "executionId",
      "userText",
      "attachments",
      "files",
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
      exact(part, ["offset", "kind", "text", "toolId", "messageId"])
      if (!Number.isSafeInteger(part.offset) || (part.offset as number) <= previousOffset)
        throw new Error("Invalid part order")
      previousOffset = part.offset as number
      const kind = text(part, "kind")
      oneOf(kind, ["text", "thought", "tool"])
      text(part, "text")
      const toolId = text(part, "toolId", 256)
      if (kind === "tool") {
        if (!toolId.length) throw new Error("Conversation tool part has no tool identity")
        toolPartIds.add(JSON.stringify([executionId, toolId]))
      } else if (toolId.length) {
        throw new Error("Conversation non-tool part has a tool identity")
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
  }
  const pendingIds = new Set<string>()
  for (const pending of items(item, "pending", 128)) {
    exact(pending, ["executionId", "text", "attachments", "files", "mode"])
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
    oneOf(text(pending, "mode"), ["queued", "steering"])
    // The waiting input and its queued message are one submission seen twice;
    // the same text over different images is as contradictory as different text.
    if (
      !item.truncated &&
      item.queueComplete &&
      (messageTexts.get(executionId) !== pendingText ||
        messageImages.get(executionId) !== pendingImages ||
        messageFiles.get(executionId) !== pendingFiles)
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
    ])
    for (const key of ["executionId", "permissionId", "toolId"]) identity(permission, key)
    const permissionKey = JSON.stringify([
      permission.executionId,
      permission.permissionId,
    ])
    if (permissionIds.has(permissionKey))
      throw new Error("Conversation response repeats a permission")
    const status = messageStatuses.get(permission.executionId as string)
    if (status !== undefined && status !== "running")
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
      exact(option, ["id", "label"])
      const id = identity(option, "id")
      if (ids.has(id)) throw new Error("Permission response repeats an option")
      ids.add(id)
      text(option, "label", 2048, false)
    }
  }
  const toolIds = new Set<string>()
  for (const tool of items(item, "tools", 128)) {
    exact(tool, ["executionId", "toolId", "title", "status", "details", "input"])
    identity(tool, "executionId")
    identity(tool, "toolId")
    const toolKey = JSON.stringify([tool.executionId, tool.toolId])
    if (toolIds.has(toolKey)) throw new Error("Conversation response repeats a tool")
    toolIds.add(toolKey)
    text(tool, "title", 2048)
    text(tool, "status", 64)
    text(tool, "details", 16384)
    text(tool, "input", 32768)
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
    exact(runtime, ["model", "provider", "workspace"])
    for (const key of ["model", "provider", "workspace"]) text(runtime, key, 4096)
  }
  const capabilities = record(item.capabilities)
  const capabilityKeys = ["queue", "steer", "resume", "permissions", "imageInput"]
  exact(capabilities, capabilityKeys)
  for (const key of capabilityKeys) flag(capabilities, key)
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
