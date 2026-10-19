import { ProductMethod } from "../generated/product.js"
import type { WorkspacePublishResult, WorkspaceWindow } from "../generated/product.js"
import type { RpcRequester } from "../application/session-port.js"

/**
 * What the window shows, told to the gateway so the person's agents can read
 * the space they share (`read_window`). The gateway keeps one window per
 * connection, only while it is open: a connection that comes back publishes
 * again.
 */
export type WorkspaceApi = {
  /**
   * Replace this connection's window with `window`, or withdraw it when
   * `window` is absent. `windowId` is the window's own UUID, the same on each
   * of its connections, so a connection it left behind is replaced rather
   * than read as a second window. `revision` counts the window's
   * publications: one lower than the revision held changes nothing and says
   * `applied: false`.
   */
  publish(
    windowId: string,
    revision: number,
    window?: WorkspaceWindow,
  ): Promise<WorkspacePublishResult>
}

export function createWorkspaceApi(session: RpcRequester): WorkspaceApi {
  return {
    publish: async (windowId, revision, window) =>
      workspacePublishResult(
        await session.request(
          ProductMethod.WorkspacePublish,
          window === undefined ? { windowId, revision } : { windowId, revision, window },
        ),
      ),
  }
}

/** The gateway's answer, checked before a caller reads it. */
function workspacePublishResult(value: unknown): WorkspacePublishResult {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid workspace publish response")
  const result = value as Record<string, unknown>
  if (
    Object.keys(result).some((key) => key !== "revision" && key !== "applied") ||
    !Number.isSafeInteger(result.revision) ||
    (result.revision as number) < 0 ||
    typeof result.applied !== "boolean"
  )
    throw new Error("Invalid workspace publish response")
  return { revision: result.revision as number, applied: result.applied }
}
