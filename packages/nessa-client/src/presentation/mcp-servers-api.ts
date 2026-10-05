import {
  NessaMcpServersError,
  type McpServersMethod,
} from "../application/mcp-servers-error.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import {
  mcpServerInspect,
  ProductMethod,
  type McpServersInspectResult,
  type McpServersListResult,
  type McpServersRemoveParams,
  type McpServersSaveParams,
  type McpServersWriteResult,
} from "../generated/product.js"
import {
  mcpServersInspectResult,
  mcpServersListResult,
  mcpServersWriteResult,
} from "../protocol/mcp-servers-validate.js"

/**
 * The gateway's stored MCP servers: list them, save one, remove one, and
 * inspect one by starting it once outside any conversation.
 *
 * Every rule a server is held to — its name, command, arguments, variables,
 * how many there may be — is the gateway's, answered as a typed refusal; the
 * client checks only that each answer is one the schema describes. Variable
 * values go out on `save` and never come back: a list names variables only.
 *
 * Every failure is a {@link NessaMcpServersError}. A save or remove that was
 * not answered may or may not have been applied, and nothing is retried for
 * you: list again to see where things stand.
 */
export type McpServersApi = {
  /** The stored servers, in stored order then the managed one, and the revision a write must name. */
  list(): Promise<McpServersListResult>
  /**
   * Store a server — add it, replace the one under its name, or rename the one
   * under `previousName` — at the revision last listed. A variable whose
   * value is `null` keeps the value stored for that name; a stored variable
   * left out is removed.
   * @returns The stored list's new revision. New conversations get the new
   * server set; running ones keep theirs.
   * @throws {@link NessaMcpServersError}: `mcp_servers_invalid` with its
   * problem, `mcp_servers_revision_conflict` with the revision now, and the
   * rest of `McpServersErrorCode`'s write refusals.
   */
  save(params: McpServersSaveParams): Promise<McpServersWriteResult>
  /**
   * Take a stored server out of the configuration, at the revision last listed.
   * @throws {@link NessaMcpServersError}, as `save`.
   */
  remove(params: McpServersRemoveParams): Promise<McpServersWriteResult>
  /**
   * Start the stored server under `name` once, list its tools and their MCP
   * Apps, and stop it. A server turned off can be inspected. The client waits
   * `mcpServerInspect.requestDeadlineMs`: the gateway's deadline and its
   * allowance for stopping the server and answering.
   * @throws {@link NessaMcpServersError}: one of the `mcp_server_` codes for a
   * server that failed, `mcp_servers_busy` when inspections are running
   * already, `mcp_servers_stopping` when the gateway is stopping,
   * `mcp_servers_not_found`, `mcp_servers_reserved_name`.
   */
  inspect(name: string): Promise<McpServersInspectResult>
}

const inspectDeadline: RequestDeadline = { atLeastMs: mcpServerInspect.requestDeadlineMs }

export function createMcpServersApi(session: RpcRequester): McpServersApi {
  async function call<T>(
    method: McpServersMethod,
    params: unknown,
    validate: (value: unknown) => T,
    deadline?: RequestDeadline,
  ): Promise<T> {
    try {
      return validate(
        deadline === undefined
          ? await session.request(method, params)
          : await session.request(method, params, deadline),
      )
    } catch (cause) {
      throw new NessaMcpServersError(method, cause)
    }
  }
  return {
    list: () => call(ProductMethod.McpServersList, {}, mcpServersListResult),
    save: (params) => call(ProductMethod.McpServersSave, params, mcpServersWriteResult),
    remove: (params) =>
      call(ProductMethod.McpServersRemove, params, mcpServersWriteResult),
    inspect: (name) =>
      call(
        ProductMethod.McpServersInspect,
        { name },
        mcpServersInspectResult,
        inspectDeadline,
      ),
  }
}
