import {
  bounds,
  McpServerKind,
  McpServerProblemCode,
  McpServersErrorCode,
  McpServersInspectCut,
  type McpInspectedTool,
  type McpServerListEntry,
  type McpServersAuditUnavailableDetails,
  type McpServersConfigTooLargeDetails,
  type McpServersInspectResult,
  type McpServersInvalidDetails,
  type McpServersListResult,
  type McpServersRevisionConflictDetails,
  type McpServersStorageUnavailableDetails,
  type McpServersWriteResult,
} from "../generated/product.js"
import { boundedName, csp, object, permissions } from "./mcp-app-validate.js"

/**
 * The answers of `mcpServers.list`, `.save`, `.remove` and `.inspect`, and
 * the details of their refusals, held to the schema's shape before anyone
 * believes them. What a server may be called, run or given is the gateway's
 * rule, and is not checked again here: an answer is checked for being one
 * the schema describes, not for agreeing with a copy of the gateway's rules.
 */

const entryKeys = ["kind", "name", "command", "args", "envNames", "enabled", "managed"]
const toolKeys = ["name", "readOnlyHint", "destructiveHint", "ui"]

/**
 * A string, as the schema says: any, the empty one included. A server stored
 * by hand under a name the gateway would refuse is still listed, so the
 * window can show it and remove it.
 */
const text = (value: unknown): value is string => typeof value === "string"
const texts = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((each) => typeof each === "string")

/**
 * Membership of a closed set of wire codes, the one way a raw code becomes
 * one of them: `Object.values` is the set's own members, so an inherited name
 * such as `constructor` is never one.
 */
function member<T extends string>(
  table: Record<string, T>,
  value: unknown,
): T | undefined {
  return typeof value === "string" && (Object.values(table) as string[]).includes(value)
    ? (value as T)
    : undefined
}

/**
 * The mcpServers refusal code this build knows by that name, or `undefined`
 * for any other string — including codes refused before dispatch, such as
 * `forbidden`, which are the session's and not these methods'.
 */
export const mcpServersErrorCode = (code: unknown): McpServersErrorCode | undefined =>
  member(McpServersErrorCode, code)

function entry(value: unknown): McpServerListEntry {
  const item = object(value, entryKeys, "MCP server entry")
  const kind = member(McpServerKind, item.kind)
  if (
    !kind ||
    !text(item.name) ||
    !text(item.command) ||
    !texts(item.args) ||
    !texts(item.envNames) ||
    typeof item.enabled !== "boolean" ||
    typeof item.managed !== "boolean"
  )
    throw new Error("Invalid MCP server entry")
  return {
    kind,
    name: item.name,
    command: item.command,
    args: [...item.args],
    envNames: [...item.envNames],
    enabled: item.enabled,
    managed: item.managed,
  }
}

/** The answer to `mcpServers.list`: a revision, and each server in the gateway's order. */
export function mcpServersListResult(value: unknown): McpServersListResult {
  const item = object(value, ["revision", "servers"], "MCP server list")
  if (!text(item.revision) || !Array.isArray(item.servers))
    throw new Error("Invalid MCP server list")
  return { revision: item.revision, servers: item.servers.map(entry) }
}

/** The answer to `mcpServers.save` and `.remove`: the stored list's revision now. */
export function mcpServersWriteResult(value: unknown): McpServersWriteResult {
  const item = object(value, ["revision"], "MCP server write")
  if (!text(item.revision)) throw new Error("Invalid MCP server write")
  return { revision: item.revision }
}

function tool(value: unknown): McpInspectedTool {
  const item = object(value, toolKeys, "inspected tool")
  if (!text(item.name)) throw new Error("Invalid inspected tool name")
  for (const hint of ["readOnlyHint", "destructiveHint"] as const)
    if (item[hint] !== undefined && typeof item[hint] !== "boolean")
      throw new Error(`Invalid inspected tool ${hint}`)
  let ui: McpInspectedTool["ui"]
  if (item.ui !== undefined) {
    const declared = object(item.ui, ["uri", "csp", "permissions"], "inspected tool UI")
    // The schema's 1 to 2048 UTF-8 bytes: the one bound every app resource URI has.
    if (!boundedName(declared.uri, bounds.maxMcpResourceUriBytes))
      throw new Error("Invalid inspected tool UI")
    ui = {
      uri: declared.uri,
      csp: csp(declared.csp),
      permissions: permissions(declared.permissions),
    }
  }
  return {
    name: item.name,
    ...(item.readOnlyHint === undefined
      ? {}
      : { readOnlyHint: item.readOnlyHint as boolean }),
    ...(item.destructiveHint === undefined
      ? {}
      : { destructiveHint: item.destructiveHint as boolean }),
    ...(ui ? { ui } : {}),
  }
}

/** The answer to `mcpServers.inspect`. `cut` is present exactly when `complete` is false. */
export function mcpServersInspectResult(value: unknown): McpServersInspectResult {
  const item = object(value, ["complete", "cut", "tools"], "inspection")
  if (typeof item.complete !== "boolean" || !Array.isArray(item.tools))
    throw new Error("Invalid inspection")
  const cut = item.cut === undefined ? undefined : member(McpServersInspectCut, item.cut)
  if (item.cut !== undefined && !cut) throw new Error("Invalid inspection cut")
  if ((cut === undefined) !== item.complete)
    throw new Error("An inspection names a cut exactly when it is incomplete")
  return { complete: item.complete, ...(cut ? { cut } : {}), tools: item.tools.map(tool) }
}

/** `mcp_servers_invalid`'s details, or undefined when they are not that shape. */
export function mcpServersInvalidDetails(
  details: unknown,
): McpServersInvalidDetails | undefined {
  try {
    const item = object(details, ["problem", "server", "name"], "invalid details")
    const problem = member(McpServerProblemCode, item.problem)
    if (
      !problem ||
      (item.server !== undefined && typeof item.server !== "string") ||
      (item.name !== undefined && typeof item.name !== "string")
    )
      return undefined
    return {
      problem,
      ...(item.server === undefined ? {} : { server: item.server as string }),
      ...(item.name === undefined ? {} : { name: item.name as string }),
    }
  } catch {
    return undefined
  }
}

/** `mcp_servers_revision_conflict`'s details, or undefined when they are not that shape. */
export function mcpServersRevisionConflictDetails(
  details: unknown,
): McpServersRevisionConflictDetails | undefined {
  try {
    const item = object(details, ["revision"], "conflict details")
    return text(item.revision) ? { revision: item.revision } : undefined
  } catch {
    return undefined
  }
}

/**
 * `mcp_servers_config_too_large`'s details from `mcpServers.list`, or
 * undefined when they are not that shape. A save refused on the same code
 * carries none, and neither does a config.json that itself passes 64 KiB.
 */
export function mcpServersConfigTooLargeDetails(
  details: unknown,
): McpServersConfigTooLargeDetails | undefined {
  try {
    const item = object(details, ["revision"], "too-large details")
    return text(item.revision) ? { revision: item.revision } : undefined
  } catch {
    return undefined
  }
}

/** `audit_unavailable`'s details from an mcpServers method, or undefined when they are not that shape. */
export function mcpServersAuditUnavailableDetails(
  details: unknown,
): McpServersAuditUnavailableDetails | undefined {
  try {
    const item = object(details, ["applied", "code"], "audit details")
    if (typeof item.applied !== "boolean") return undefined
    const code = item.code === undefined ? undefined : mcpServersErrorCode(item.code)
    // Never audit_unavailable, says the schema: a code that is, or that this
    // build does not know, leaves the details without it rather than wrong.
    if (
      item.code !== undefined &&
      (!code || code === McpServersErrorCode.AuditUnavailable)
    )
      return { applied: item.applied }
    return { applied: item.applied, ...(code ? { code } : {}) }
  } catch {
    return undefined
  }
}

/** `mcp_servers_storage_unavailable`'s details, or undefined when they are not that shape. */
export function mcpServersStorageUnavailableDetails(
  details: unknown,
): McpServersStorageUnavailableDetails | undefined {
  try {
    const item = object(details, ["applied"], "storage details")
    return typeof item.applied === "boolean" ? { applied: item.applied } : undefined
  } catch {
    return undefined
  }
}
