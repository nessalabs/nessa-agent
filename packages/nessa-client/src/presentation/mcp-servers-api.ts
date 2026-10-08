import {
  NessaMcpServersError,
  type McpServersMethod,
} from "../application/mcp-servers-error.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import {
  mcpServerInspect,
  productMethodGrants,
  ProductMethod,
  type McpServersAuthorizeParams,
  type McpServersAuthorizeResult,
  type McpServersInspectResult,
  type McpServersListResult,
  type McpServersRemoveParams,
  type McpServersRevokeParams,
  type McpServersRevokeResult,
  type McpServersSaveParams,
  type McpServersWriteResult,
  type ProductSessionReady,
} from "../generated/product.js"
import {
  mcpServersAuthorizeResult,
  mcpServersInspectResult,
  mcpServersListResult,
  mcpServersRevokeResult,
  mcpServersWriteResult,
} from "../protocol/mcp-servers-validate.js"

// The methods this API calls, named by the published table rather than listed again.
const mcpServersGrants = Object.entries(productMethodGrants).flatMap(([method, grant]) =>
  method.startsWith("mcpServers.") ? [grant] : [],
)
const mcpServersGrant = mcpServersGrants[0]
if (
  mcpServersGrants.length === 0 ||
  typeof mcpServersGrant !== "string" ||
  mcpServersGrants.some((grant) => grant !== mcpServersGrant)
)
  throw new Error("mcpServers methods do not share one published grant")

/**
 * Whether `session` carries the grant every mcpServers method asks for,
 * for this connected gateway.
 *
 * The action is `productMethodGrants`, generated from
 * `protocol/product/manifest.json`. The resource is the session's
 * organization and gateway id, the same pair Cedar compares before it
 * allows the action. Carrying it is not permission: the gateway can still
 * answer `forbidden`, and that answer is the one a surface goes by.
 */
export function carriesMcpServersGrant(
  session: Pick<ProductSessionReady, "grants" | "gatewayId" | "organizationId">,
): boolean {
  return session.grants.some(
    (grant) =>
      grant.action === mcpServersGrant &&
      grant.resource.organizationId === session.organizationId &&
      grant.resource.id === session.gatewayId,
  )
}

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
   * @returns The stored list's new revision, and `live`: whether new
   * conversations get the list as now written (running ones keep theirs).
   * `live` is false when the gateway is stopping, and the next start reads
   * the stored list.
   * @throws {@link NessaMcpServersError}: `mcp_servers_invalid` with its
   * problem, `mcp_servers_revision_conflict` with the revision now, and the
   * rest of `McpServersErrorCode`'s write refusals.
   */
  save(params: McpServersSaveParams): Promise<McpServersWriteResult>
  /**
   * Take a stored server out of the configuration, at the revision last listed.
   * @returns As `save`. `live` is also false when the rest of the list, as
   * edited by hand, is still past a bound: the removed server is out of new
   * conversations all the same, and the rest stay as they were.
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
  /**
   * Begin consent for the remote server `id` at the revision last listed.
   * `pending_consent` carries the URL the host opens. No token is returned.
   * @throws {@link NessaMcpServersError} when the store, discovery, or the
   * revision refuses the request.
   */
  authorize(params: McpServersAuthorizeParams): Promise<McpServersAuthorizeResult>
  /**
   * Fence the remote server `id`. `settled` is false while drain, deletion,
   * remote observation, or evidence is still outstanding.
   */
  revoke(params: McpServersRevokeParams): Promise<McpServersRevokeResult>
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
    authorize: (params) =>
      call(ProductMethod.McpServersAuthorize, params, mcpServersAuthorizeResult),
    revoke: (params) =>
      call(ProductMethod.McpServersRevoke, params, mcpServersRevokeResult),
  }
}
