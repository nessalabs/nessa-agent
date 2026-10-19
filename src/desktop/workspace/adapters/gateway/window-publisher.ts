/**
 * The `WindowPublisher` over the window's gateway client: what the window
 * shows, told with `client.workspace.publish`, for the person's agents to
 * read (`read_window`). The gateway keeps it only while this connection is
 * open, so it is said again each time the connection comes back.
 *
 * ```text
 * publish(window) ─▶ latest ─▶ flush: one publication at a time, the newest last
 *   connected client ─▶ workspace.publish(windowId, revision + 1, wire(latest))
 *   the connection back (`connected`) ─▶ flush again
 *   closed, no client, or refused for now (`temporarily_unavailable`…) ─▶ again after retryMs
 *   refused for good (`invalid_request`…) ─▶ logged; a withdrawal is owed in
 *     its place, sent like any window, until the window changes
 * ```
 *
 * `windowId` is this window's for its whole life, and its revisions go on
 * rising across connections, so the gateway reads a connection it left
 * behind as the same window, older.
 *
 * A session's id is its conversation's (`gateway-source.ts`). An id that is
 * not a conversation's — a sample's — is said as a draft: nothing the
 * gateway could look up.
 */
import {
  NessaRpcError,
  type ConnectionState,
  type WorkspaceApi,
  type WorkspacePaneItem,
  type WorkspaceWindow,
} from "@nessa/client"
import type { WindowPublisher } from "../../application/ports"
import type { ShownItem, ShownWindow } from "../../model/shown-window"

/** What publishing asks of the window's gateway client. */
export interface WindowPublishClient {
  readonly workspace: Pick<WorkspaceApi, "publish">
  onConnectionStateChange(handler: (state: ConnectionState) => void): () => void
}

/**
 * Refusals that say the window itself is wrong, or that this gateway takes
 * none: sending it again would be refused again. Any other refusal is
 * for now, and the window is sent again.
 */
const refusedForGood = new Set([
  "invalid_request",
  "forbidden",
  "unknown_method",
  "conversations_not_configured",
])

const conversationId = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

export function windowPublisher(options: {
  connected: () => Promise<WindowPublishClient>
  after: (ms: number, run: () => void) => () => void
  retryMs: number
  /** This window's own id; a new one by default. */
  windowId?: string
}): WindowPublisher {
  const windowId = options.windowId ?? crypto.randomUUID()
  // `undefined` until the window has said anything.
  let latest: ShownWindow | null | undefined
  let revision = 0
  let sending = false
  let owed = false
  let watched: WindowPublishClient | undefined
  let unwatch: (() => void) | undefined
  let cancelRetry: (() => void) | undefined

  const retry = () => {
    if (cancelRetry) return
    cancelRetry = options.after(options.retryMs, () => {
      cancelRetry = undefined
      void flush()
    })
  }
  /** Says it again on this client's connection coming back, or on the next one. */
  const watch = (client: WindowPublishClient) => {
    if (watched === client) return
    unwatch?.()
    watched = client
    unwatch = client.onConnectionStateChange((state) => {
      if (state.status === "connected") void flush()
      else if (state.status === "closed") retry()
    })
  }
  const flush = async (): Promise<void> => {
    if (latest === undefined) return
    if (sending) {
      owed = true
      return
    }
    sending = true
    try {
      do {
        owed = false
        const client = await options.connected()
        watch(client)
        const window = latest
        revision += 1
        try {
          await client.workspace.publish(
            windowId,
            revision,
            window ? wire(window) : undefined,
          )
        } catch (error) {
          if (!(error instanceof NessaRpcError && refusedForGood.has(error.code)))
            throw error
          console.warn("The gateway refused what the window shows", error)
          // What it held before is not what the window shows now: withdrawn,
          // sent and sent again like any window, unless the window changes.
          if (window && latest === window) {
            latest = null
            owed = true
          }
        }
      } while (owed)
    } catch {
      retry()
    } finally {
      sending = false
    }
    // A change that came while a refused or failed publication was out.
    if (owed) void flush()
  }
  return {
    publish(window) {
      latest = window
      void flush()
    },
  }
}

/** The window in the protocol's words. */
function wire(window: ShownWindow): WorkspaceWindow {
  return {
    arrangement: window.arrangement,
    sidebar: window.sidebar,
    sessionList: window.sessionList,
    showing: window.showing.kind,
    ...(window.showing.kind === "widget"
      ? { widget: { plugin: window.showing.widget.plugin, id: window.showing.widget.id } }
      : {}),
    columns: window.columns.map((column) => ({
      widthPercent: column.widthPercent,
      panes: column.panes.map((pane) => ({
        heightPercent: pane.heightPercent,
        focused: pane.focused,
        item: wireItem(pane.item),
      })),
    })),
  }
}

function wireItem(item: ShownItem): WorkspacePaneItem {
  switch (item.kind) {
    case "conversation":
      return conversationId.test(item.sessionId)
        ? { kind: "conversation", conversationId: item.sessionId }
        : { kind: "draft" }
    case "draft":
      return { kind: "draft" }
    case "widget":
      return {
        kind: "widget",
        widget: { plugin: item.widget.plugin, id: item.widget.id },
      }
  }
}
