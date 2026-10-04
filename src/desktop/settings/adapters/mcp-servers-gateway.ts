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
  type McpServerProblemCode,
  type McpServersApi,
  type McpServersErrorCode,
  type McpServersInspectResult,
  type McpServersListResult,
  type McpServersRefusal,
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
type Detailed = "mcp_servers_invalid" | "audit_unavailable" | "mcp_server_remote_error"

const refusalCodes: Record<
  Exclude<McpServersErrorCode, Detailed>,
  Exclude<RefusalCode, "invalid" | "auditUnavailable" | "remoteError">
> = {
  mcp_servers_not_configured: "notConfigured",
  mcp_servers_reserved_name: "reservedName",
  mcp_servers_not_found: "notFound",
  mcp_servers_revision_conflict: "revisionConflict",
  mcp_servers_busy: "busy",
  mcp_servers_config_invalid: "configInvalid",
  mcp_servers_config_too_large: "configTooLarge",
  mcp_servers_storage_unavailable: "storageUnavailable",
  mcp_server_start_failed: "startFailed",
  mcp_server_timed_out: "timedOut",
  mcp_server_gone: "gone",
  mcp_server_malformed: "malformed",
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
              ...(refusal.details.code ? { code: refusal.details.code } : {}),
            }
          : {}),
      }
    case "mcp_server_remote_error":
      return { kind: "remoteError", ...(refusal.details ?? {}) }
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

function inspectedTool(tool: McpInspectedTool): InspectedTool {
  return {
    name: tool.name,
    ...(tool.readOnlyHint === undefined ? {} : { readOnly: tool.readOnlyHint }),
    ...(tool.destructiveHint === undefined ? {} : { destructive: tool.destructiveHint }),
    ...(tool.ui
      ? {
          ui: {
            uri: tool.ui.uri,
            csp: [
              { name: "connect", origins: tool.ui.csp.connectDomains },
              { name: "resource", origins: tool.ui.csp.resourceDomains },
              { name: "frame", origins: tool.ui.csp.frameDomains },
              { name: "base-uri", origins: tool.ui.csp.baseUriDomains },
            ].filter((each) => each.origins.length > 0),
            permissions: (
              ["camera", "microphone", "geolocation", "clipboardWrite"] as const
            ).filter((permission) => tool.ui?.permissions[permission] === true),
          },
        }
      : {}),
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
