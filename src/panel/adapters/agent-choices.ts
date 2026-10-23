import type { AgentsListResult, NessaClient } from "@nessa/client"

/** What the panel's pickers offer: the gateway's agent catalog, and the agent
 * setup chose, when somebody has. */
export type ConversationChoices = {
  catalog: AgentsListResult
  chosenAgent: string | undefined
}

/**
 * Hold the choices while the panel shows them: `show` is told the choices as
 * they are now, and again each time the catalog changes. Answers the stop,
 * after which `show` is told nothing more.
 */
export type FollowConversationChoices = (
  show: (choices: ConversationChoices) => void,
) => () => void

/**
 * Follow the gateway's agent catalog (`agents.subscribe`): it is sent first
 * and again whenever a paired environment connects, disconnects or is
 * revoked, so "Run on" offers the places a conversation can start on now
 * rather than the ones there were when the panel opened.
 *
 * A subscription that ends lagging is opened again at once; one that could
 * not open or ended for any other reason, the connection lost included, is
 * opened again once `retry` resolves, the pace the panel's other follows
 * keep. The choices last shown stay shown meanwhile. Stopped, the
 * subscription is given up (closed now, or once its open is answered) and
 * nothing it says is shown.
 */
export function followAgentChoices(
  client: () => NessaClient | null,
  /** Resolves when it is time to subscribe again. Composition supplies the
   * real clock; tests resolve it by hand. */
  retry: () => Promise<void>,
  /** The agent setup chose, asked again for each catalog until somebody has
   * chosen. */
  chosenAgent: () => Promise<string | undefined>,
  /** Tells the listener when setup recorded a choice — a replay can finish on
   * another agent while the panel runs, with no new catalog to carry it — and
   * answers the way to stop listening. */
  setupRecorded: (listener: () => void) => () => void = () => () => undefined,
): FollowConversationChoices {
  return (show) => {
    let stopped = false
    // The catalog last shown, so a recorded choice is shown against it.
    let shown: AgentsListResult | undefined
    // The open whose frames apply: anything an earlier open brings is dropped.
    let token = {}
    let giveUp: AbortController | undefined
    // Which catalog is the newest: the agent is asked for each, and an
    // older catalog whose answer comes later is not shown over a newer one.
    let newest = 0
    const again = () => {
      token = {}
      if (!stopped) void retry().then(open)
    }
    const open = (): void => {
      if (stopped) return
      const mine = (token = {})
      giveUp?.abort()
      const { signal } = (giveUp = new AbortController())
      const current = () => !stopped && token === mine
      void Promise.resolve()
        .then(() => {
          const now = client()
          if (!now || (now.connectionState && now.connectionState.status !== "connected"))
            throw new Error("The gateway is not connected")
          return now.subscriptions.agents(
            {
              catalog: (catalog) => {
                if (current()) choose(catalog, current)
              },
              ended: (end) => {
                if (!current()) return
                // Too slow to take a frame: on at once, with a fresh catalog.
                if (end.reason === "lagging") return open()
                again()
              },
            },
            { signal },
          )
        })
        .then(
          (opened) => {
            if (!current()) void opened.close().catch(() => undefined)
          },
          () => {
            if (current()) again()
          },
        )
    }
    // The agent is asked for each catalog, and shown with it unless a newer
    // catalog or choice was asked about since.
    const choose = (catalog: AgentsListResult, still: () => boolean) => {
      const order = ++newest
      void chosenAgent().then(
        (agent) => {
          if (still() && order === newest) {
            shown = catalog
            show({ catalog, chosenAgent: agent })
          }
        },
        // Not an answer: the next catalog asks again.
        () => undefined,
      )
    }
    const unlisten = setupRecorded(() => {
      if (!stopped && shown) choose(shown, () => !stopped)
    })
    open()
    return () => {
      stopped = true
      giveUp?.abort()
      unlisten()
    }
  }
}
