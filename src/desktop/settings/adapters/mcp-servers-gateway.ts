/**
 * The gateway's stored MCP servers, for Settings › Integrations: the window's
 * client (`client.mcpServers`, `client.productSession`) read into the
 * model's words (`model/mcp-servers.ts`). Each refusal code and problem is
 * mapped by a total table, so a code the protocol adds is a type error here
 * rather than a sentence nobody wrote; a code this build does not know never
 * reaches the table (`NessaMcpServersError` narrows by membership).
 */
import {
  mayManageMcpServers,
  mcpServerInspect,
  McpServerKind,
  NessaMcpServersError,
  type ConnectionState,
  type McpInspectedTool,
  type McpInspectedUi,
  type McpServerProblemCode,
  type McpServersApi,
  type McpServersErrorCode,
  type McpServersInspectResult,
  type McpServersListResult,
  type McpServersRefusal,
  type McpUiCsp,
  type McpUiPermissions,
  type ProductSessionReady,
} from "@nessa/client"
import type {
  Failure,
  InspectedTool,
  Inspection,
  McpServersLimits,
  Outcome,
  Problem,
  RefusalCode,
  RemoveRequest,
  SaveRequest,
  ServerList,
} from "../model/mcp-servers"

/** What Settings asks of the window's gateway client. */
export interface McpServersClient {
  readonly mcpServers: McpServersApi
  readonly productSession: ProductSessionReady
  readonly connectionState: ConnectionState
  onConnectionStateChange(handler: (state: ConnectionState) => void): () => void
}

/** How the connection stands, as the tab follows it. */
export type McpServersConnection =
  | { readonly type: "connected"; readonly mayManage: boolean }
  | { readonly type: "unreachable" }

/** The stored servers, as Settings › Integrations manages them. */
export interface McpServersGateway {
  readonly limits: McpServersLimits
  /** Tells `handler` how the connection stands, now and on each change, until the returned stop. */
  follow(handler: (connection: McpServersConnection) => void): () => void
  list(): Promise<Outcome<ServerList>>
  save(request: SaveRequest): Promise<Outcome<void>>
  remove(request: RemoveRequest): Promise<Outcome<void>>
  inspect(name: string): Promise<Outcome<Inspection>>
}

/** The refusals that carry details, each read with them in `refused`. */
type Detailed =
  | "mcp_servers_invalid"
  | "audit_unavailable"
  | "mcp_servers_storage_unavailable"
  | "mcp_server_remote_error"
  | "mcp_servers_config_too_large"

const refusalCodes: Record<
  Exclude<McpServersErrorCode, Detailed>,
  Exclude<
    RefusalCode,
    | "invalid"
    | "auditUnavailable"
    | "storageUnavailable"
    | "remoteError"
    | "configTooLarge"
  >
> = {
  mcp_servers_not_configured: "notConfigured",
  mcp_servers_reserved_name: "reservedName",
  mcp_servers_not_found: "notFound",
  mcp_servers_revision_conflict: "revisionConflict",
  mcp_servers_busy: "busy",
  mcp_servers_stopping: "stopping",
  mcp_servers_config_invalid: "configInvalid",
  mcp_server_start_failed: "startFailed",
  mcp_server_timed_out: "timedOut",
  mcp_server_gone: "gone",
  mcp_server_malformed: "malformed",
}

/** Every code in the window's words: what an audit refusal says stopped its request. */
const causeCodes: Record<McpServersErrorCode, RefusalCode> = {
  ...refusalCodes,
  mcp_servers_invalid: "invalid",
  audit_unavailable: "auditUnavailable",
  mcp_servers_storage_unavailable: "storageUnavailable",
  mcp_server_remote_error: "remoteError",
  mcp_servers_config_too_large: "configTooLarge",
}

const problems: Record<McpServerProblemCode, Problem> = {
  too_many: "tooMany",
  duplicate_name: "duplicateName",
  name: "name",
  command: "command",
  arguments: "arguments",
  environment_name: "environmentName",
  reserved_environment_name: "reservedEnvironmentName",
  environment_value: "environmentValue",
  environment_value_missing: "environmentValueMissing",
  environment_name_repeated: "environmentNameRepeated",
}

function refused(refusal: McpServersRefusal): Failure {
  switch (refusal.code) {
    case "mcp_servers_invalid":
      return {
        kind: "invalid",
        ...(refusal.details
          ? {
              problem: problems[refusal.details.problem],
              ...(refusal.details.server === undefined
                ? {}
                : { server: refusal.details.server }),
              ...(refusal.details.name === undefined
                ? {}
                : { name: refusal.details.name }),
            }
          : {}),
      }
    case "audit_unavailable":
      return {
        kind: "auditUnavailable",
        ...(refusal.details
          ? {
              applied: refusal.details.applied,
              ...(refusal.details.code
                ? { cause: causeCodes[refusal.details.code] }
                : {}),
            }
          : {}),
      }
    case "mcp_servers_storage_unavailable":
      return {
        kind: "storageUnavailable",
        ...(refusal.details ? { applied: refusal.details.applied } : {}),
      }
    case "mcp_server_remote_error":
      return { kind: "remoteError", ...(refusal.details ?? {}) }
    case "mcp_servers_config_too_large":
      // A list refused so names the revision a remove by name needs (U44).
      return {
        kind: "configTooLarge",
        ...(refusal.details ? { revision: refusal.details.revision } : {}),
      }
    default:
      return { kind: refusalCodes[refusal.code] }
  }
}

/** What a failed call means to the tab. */
export function failureOf(error: unknown): Failure {
  if (!(error instanceof NessaMcpServersError)) return { kind: "unanswered" }
  if (error.forbidden) return { kind: "forbidden" }
  return error.refusal ? refused(error.refusal) : { kind: "unanswered" }
}

function serverList(result: McpServersListResult): ServerList {
  return {
    revision: result.revision,
    servers: result.servers.map(
      ({ name, command, args, envNames, enabled, managed }) => ({
        name,
        command,
        args,
        envNames,
        enabled,
        managed,
      }),
    ),
  }
}

/**
 * Each CSP list's name as the panel shows it, in the order shown: total over
 * the protocol's lists, so a list it adds is a type error here, not one the
 * panel leaves out.
 */
const cspNames: Record<keyof McpUiCsp, string> = {
  connectDomains: "connect",
  resourceDomains: "resource",
  frameDomains: "frame",
  baseUriDomains: "base-uri",
}

/** Each permission's name as the panel shows it; total, as `cspNames`. */
const permissionNames: Record<keyof McpUiPermissions, string> = {
  camera: "camera",
  microphone: "microphone",
  geolocation: "geolocation",
  clipboardWrite: "clipboardWrite",
}

/** A table's own keys, in its order, typed as the table's. */
const keysOf = <K extends string>(table: Record<K, string>) => Object.keys(table) as K[]

function inspectedUi(ui: McpInspectedUi): NonNullable<InspectedTool["ui"]> {
  return {
    uri: ui.uri,
    csp: keysOf(cspNames)
      .map((key) => ({ name: cspNames[key], origins: ui.csp[key] }))
      .filter((each) => each.origins.length > 0),
    permissions: keysOf(permissionNames)
      .filter((key) => ui.permissions[key])
      .map((key) => permissionNames[key]),
  }
}

function inspectedTool(tool: McpInspectedTool): InspectedTool {
  return {
    name: tool.name,
    ...(tool.readOnlyHint === undefined ? {} : { readOnly: tool.readOnlyHint }),
    ...(tool.destructiveHint === undefined ? {} : { destructive: tool.destructiveHint }),
    ...(tool.ui ? { ui: inspectedUi(tool.ui) } : {}),
  }
}

function inspection(result: McpServersInspectResult): Inspection {
  return {
    complete: result.complete,
    ...(result.cut ? { cut: result.cut } : {}),
    tools: result.tools.map(inspectedTool),
  }
}

async function outcome<T>(run: () => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await run() }
  } catch (error) {
    return { ok: false, failure: failureOf(error) }
  }
}

/**
 * The servers over the window's gateway client. `connected` is the window's
 * one client, connecting on first need, rejecting when none connects in time;
 * while there is none, the connection is retried every `retryMs`.
 */
export function mcpServersGateway(options: {
  readonly connected: () => Promise<McpServersClient>
  readonly after: (ms: number, run: () => void) => () => void
  readonly retryMs?: number
}): McpServersGateway {
  const { connected, after } = options
  const retryMs = options.retryMs ?? 2_000
  const api = () => connected().then((client) => client.mcpServers)
  return {
    limits: { inspectDeadlineMs: mcpServerInspect.deadlineMs },
    follow(handler) {
      let stopped = false
      let off = () => {}
      let cancelRetry = () => {}
      const retry = () => {
        cancelRetry = after(retryMs, attempt)
      }
      function attempt() {
        connected().then(
          (client) => {
            if (stopped) return
            const tell = (state: ConnectionState) => {
              if (stopped) return
              if (state.status === "connected") {
                handler({
                  type: "connected",
                  mayManage: mayManageMcpServers(client.productSession),
                })
                return
              }
              handler({ type: "unreachable" })
              // Gone for good: the window's next client is another one.
              if (state.status === "closed") {
                off()
                retry()
              }
            }
            off = client.onConnectionStateChange(tell)
            tell(client.connectionState)
          },
          () => {
            if (stopped) return
            handler({ type: "unreachable" })
            retry()
          },
        )
      }
      attempt()
      return () => {
        stopped = true
        off()
        cancelRetry()
      }
    },
    list: () => outcome(async () => serverList(await (await api()).list())),
    save: (request) =>
      outcome(async () => {
        await (
          await api()
        ).save({
          revision: request.revision,
          ...(request.previousName === undefined
            ? {}
            : { previousName: request.previousName }),
          server: {
            kind: McpServerKind.Stdio,
            name: request.server.name,
            command: request.server.command,
            args: [...request.server.args],
            env: request.server.env.map(({ name, value }) => ({ name, value })),
            enabled: request.server.enabled,
          },
        })
      }),
    remove: (request) =>
      outcome(async () => {
        await (await api()).remove({ revision: request.revision, name: request.name })
      }),
    inspect: (name) => outcome(async () => inspection(await (await api()).inspect(name))),
  }
}
