import type {
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationReorderResult,
} from "../generated/product.js"

const utf8 = new TextEncoder()

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid conversation response")
  return value as Record<string, unknown>
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
function oneOf(value: string, values: string[]) {
  if (!values.includes(value)) throw new Error("Invalid conversation state")
}

export function conversationId(
  value: unknown,
  expected: string,
): { conversationId: string } {
  const item = record(value)
  if (identity(item, "conversationId") !== expected)
    throw new Error("Conversation response belongs to another conversation")
  return { conversationId: expected }
}
export function conversationReceipt(
  value: unknown,
  executionId: string,
): ConversationReceipt {
  const item = record(value)
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
  if (identity(item, "requestId") !== requestId)
    throw new Error("Conversation receipt belongs to another action")
  flag(item, "applied")
  return item as unknown as ConversationMutationResult
}
export function conversationView(value: unknown, expected: string): ConversationView {
  conversationId(value, expected)
  const item = record(value)
  identity(item, "revision")
  flag(item, "truncated")
  flag(item, "queueComplete")
  if (item.permissionViewError !== undefined)
    text(item, "permissionViewError", 2048, false)
  const messages = items(item, "messages", 128)
  const messageIds = new Set<string>()
  const messageStatuses = new Map<string, string>()
  for (const message of messages) {
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
      if (!Number.isSafeInteger(part.offset) || (part.offset as number) <= previousOffset)
        throw new Error("Invalid part order")
      previousOffset = part.offset as number
      oneOf(text(part, "kind"), ["text", "thought", "tool"])
      text(part, "text")
      text(part, "toolId", 256)
      if (part.messageId !== undefined) identity(part, "messageId")
    }
    text(message, "userText")
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
  }
  const pendingIds = new Set<string>()
  for (const pending of items(item, "pending", 128)) {
    const executionId = identity(pending, "executionId")
    if (pendingIds.has(executionId))
      throw new Error("Conversation response repeats a pending execution")
    const status = messageStatuses.get(executionId)
    if (status !== undefined && status !== "queued")
      throw new Error("Pending execution is not queued")
    if (status === undefined && !item.truncated)
      throw new Error("Pending execution is missing its message")
    pendingIds.add(executionId)
    text(pending, "text", 8192)
    oneOf(text(pending, "mode"), ["queued", "steering"])
  }
  const permissionIds = new Set<string>()
  for (const permission of items(item, "permissions", 64)) {
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
      const id = identity(option, "id")
      if (ids.has(id)) throw new Error("Permission response repeats an option")
      ids.add(id)
      text(option, "label", 2048, false)
    }
  }
  const toolIds = new Set<string>()
  for (const tool of items(item, "tools", 128)) {
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
  if (item.runtime !== undefined) {
    const runtime = record(item.runtime)
    for (const key of ["model", "provider", "workspace"]) text(runtime, key, 4096)
  }
  const capabilities = record(item.capabilities)
  for (const key of ["queue", "steer", "resume", "permissions"]) flag(capabilities, key)
  return item as unknown as ConversationView
}

export function conversationReorder(
  value: unknown,
  requestId: string,
): ConversationReorderResult {
  const item = record(value)
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
