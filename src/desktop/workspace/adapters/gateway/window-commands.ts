/**
 * The `WindowCommandInbox` over the window's gateway client: the changes the
 * person's agents, or their CLI, ask of this window (`workspace.command`),
 * handed to the window to carry out, and answered once the gateway holds
 * what the window shows after (`workspace.answer`).
 *
 * ```text
 * workspace.command { windowId, commandId, action }
 *   ─▶ for another window, or not a change this window knows ─▶ not answered (the gateway times out)
 *   ─▶ carryOut(change) ─▶ refused { code } ─────────────────────────────▶ answer refused
 *                      └─▶ applied | unchanged ─▶ publisher.settled() ─▶ answer with that revision
 * ```
 *
 * Changes run one at a time, in the order sent, so each is measured against
 * the window the one before it left.
 *
 * The gateway sends a change to the connection that holds the window's
 * newest publication, so the inbox listens on each client the publisher
 * publishes on (`clientsSeen`), as it gets it and before it publishes: a
 * connection that closes and comes back as a new client is listened on
 * again, and a window that could not connect at first starts listening when
 * it does.
 */
import type {
  WorkspaceAction,
  WorkspaceAnswerParams,
  WorkspaceApi,
  WorkspaceCommand,
} from "@nessa/client"
import type { WindowCommandInbox, WindowPublisher } from "../../application/ports"
import type { WindowChange, WindowChangeOutcome } from "../../model/window-change"

/** What the inbox asks of the window's gateway client. */
export interface WindowCommandClient {
  readonly workspace: Pick<WorkspaceApi, "answer">
  on(event: "workspace.command", handler: (command: WorkspaceCommand) => void): () => void
}

const conversationId = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

/**
 * `connected`, telling `follow`'s listeners about each new client it gives,
 * and a listener that comes late about the latest one.
 */
export function clientsSeen<C>(connected: () => Promise<C>): {
  connected: () => Promise<C>
  follow: (listener: (client: C) => void) => () => void
} {
  const listeners = new Set<(client: C) => void>()
  let latest: C | undefined
  return {
    connected: async () => {
      const client = await connected()
      if (client !== latest) {
        latest = client
        for (const listener of listeners) listener(client)
      }
      return client
    },
    follow(listener) {
      listeners.add(listener)
      if (latest !== undefined) listener(latest)
      return () => listeners.delete(listener)
    },
  }
}

export function windowCommandInbox(options: {
  /** Each client the publisher publishes on, before it publishes (`clientsSeen`). */
  clients: (listener: (client: WindowCommandClient) => void) => () => void
  publisher: WindowPublisher
}): WindowCommandInbox {
  return {
    listen(carryOut) {
      let stopped = false
      let unsubscribe: (() => void) | undefined
      // One change at a time, in the order sent.
      let queue: Promise<void> = Promise.resolve()
      const answer = async (
        client: WindowCommandClient,
        command: WorkspaceCommand,
        outcome: WindowChangeOutcome,
      ) => {
        const base = { windowId: command.windowId, commandId: command.commandId }
        const said: WorkspaceAnswerParams =
          outcome.kind === "refused"
            ? { ...base, outcome: "refused", code: outcome.code }
            : {
                ...base,
                outcome: outcome.kind,
                revision: await options.publisher.settled(),
              }
        await client.workspace.answer(said)
      }
      const unfollow = options.clients((client) => {
        if (stopped) return
        unsubscribe?.()
        unsubscribe = client.on("workspace.command", (command) => {
          if (command.windowId !== options.publisher.windowId) return
          const change = windowChange(command.action)
          if (!change) return
          queue = queue
            .then(() => carryOut(change))
            .then((outcome) => answer(client, command, outcome))
            .catch((error: unknown) => {
              // Unanswered, the gateway tells the agent the window did not answer.
              console.warn("A change asked of the window was not answered", error)
            })
        })
      })
      return () => {
        stopped = true
        unfollow()
        unsubscribe?.()
      }
    },
  }
}

/** The change in the window's own words, or `null` when it is not one it knows. */
export function windowChange(action: WorkspaceAction): WindowChange | null {
  const session = (id: string | undefined) =>
    id !== undefined && conversationId.test(id) ? id : null
  switch (action.kind) {
    case "sidebar":
    case "panel":
      return action.open === undefined ? null : { kind: action.kind, open: action.open }
    case "session_list":
      return action.open === undefined ? null : { kind: "sessionList", open: action.open }
    case "panel_tab":
      return action.widget ? { kind: "panelTab", widget: action.widget } : null
    case "show":
      return action.view ? { kind: "show", view: action.view } : null
    case "open_conversation": {
      const sessionId = session(action.conversationId)
      return sessionId
        ? { kind: "open", sessionId, place: action.place ?? "beside" }
        : null
    }
    case "focus_pane":
    case "close_pane": {
      const sessionId = session(action.conversationId)
      if (!sessionId) return null
      return { kind: action.kind === "focus_pane" ? "focus" : "close", sessionId }
    }
    default:
      return null
  }
}
