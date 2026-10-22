/**
 * Clients that only follow, for the conversation stress check
 * (`conversation-stress.mjs`). Each holds as many conversation subscriptions
 * as one connection may (`subscriptionLimits`): one view of the main
 * conversation and the events of every other conversation it finds through
 * its list subscription. As it goes it stops one at random and follows it
 * again from the last cursor it applied, asks for one more than it may hold
 * (and must be refused), and follows again after a restart drops its
 * connection, a change source closes, or the gateway ends a stream it fell
 * behind on.
 *
 * What it must show at the end: every conversation's events, as it applied
 * them, are exactly the conversation's events from the start (none missed,
 * none twice, none out of order), and its view of the main conversation is
 * the one a read gives.
 */

/**
 * How a subscription may end without anything being wrong.
 * `source_closed` is the change source closing while the socket is still
 * up: a restart drains the watches and can deliver this before the socket
 * dies. The product protocol says subscribe again, as for `disconnected`
 * and `lagging`. Leaving it fatal freezes the view and, because the server
 * has already released the slot, the next probe is accepted past the limit.
 */
const RESUMED = new Set(["disconnected", "lagging", "source_closed"])
/** Refusals that say this follower asked for something it should not have. */
const REFUSED = new Set(["subscription_capacity", "subscription_duplicate"])

/** Whether `error` is the gateway's answer, not a dropped connection. */
const isRefusal = (error) =>
  typeof error?.code === "string" &&
  !["disconnected", "connection_closed", "session_closed"].includes(error.code)

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** Replay until its applied checkpoint equals a freshly captured committed head. */
export async function replayEvents(client, conversationId) {
  const ids = []
  const controller = new AbortController()
  const { signal } = controller
  const opened = new Set()
  let ended = null
  let phase = "replay to checkpoint"
  let timer
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      const failure = new Error(
        `a replay of ${conversationId} did not complete ${phase} within 60000 ms`,
      )
      controller.abort(failure)
      reject(failure)
    }, 60_000)
  })
  const open = async (handlers, options = {}) => {
    const subscription = await client.subscriptions.events(conversationId, handlers, {
      ...options,
      signal,
    })
    if (signal.aborted) {
      void subscription.close().catch(() => {})
      signal.throwIfAborted()
    }
    opened.add(subscription)
    return subscription
  }
  const wait = async (complete) => {
    while (!complete() && !ended && !signal.aborted) await sleep(100)
    signal.throwIfAborted()
    if (ended) throw new Error(`a replay of ${conversationId} ended ${ended.reason}`)
  }
  const replay = async () => {
    let after
    while (true) {
      signal.throwIfAborted()
      phase = "replay to checkpoint"
      let candidate
      const subscription = await open(
        {
          batch(batch) {
            if (signal.aborted || candidate !== undefined) return
            for (const event of batch.events) ids.push(event.id)
            // Empty is only a candidate, not proof of reaching the current head.
            if (batch.events.length === 0) candidate = batch.cursor
          },
          ended(end) {
            ended = end
          },
        },
        after === undefined ? { replay: true } : { after },
      )
      await wait(() => candidate !== undefined)
      phase = "replay cleanup"
      await subscription.close()
      opened.delete(subscription)
      signal.throwIfAborted()
      phase = "head capture"
      let head
      // Default Now emits the certified outer-save head. Capture it after the
      // candidate: idle parking can write even after all turns are at rest.
      const capture = await open({
        batch(batch) {
          head ??= batch.cursor
        },
        ended(end) {
          ended = end
        },
      })
      await wait(() => head !== undefined)
      phase = "head capture cleanup"
      await capture.close()
      opened.delete(capture)
      if (candidate === head) return ids
      // Freeze callbacks at the applied candidate and resume strictly after it.
      // A growing head shares the same overall deadline, never a fresh budget.
      after = candidate
    }
  }
  try {
    return await Promise.race([replay(), deadline])
  } finally {
    clearTimeout(timer)
    controller.abort()
    // The client's abort gate owns late opens and their physical close ACKs.
    // Do not let a stalled unsubscribe extend this verifier's overall deadline.
    for (const subscription of opened) void subscription.close().catch(() => {})
  }
}

export class Follower {
  /**
   * `connect` makes a new client; `limits` is the published
   * `subscriptionLimits`; `random` the run's seeded generator.
   */
  constructor({ n, connect, limits, random, mainId, deleted = new Set() }) {
    this.n = n
    /** Conversations the run deleted: their streams end, and they are let go. */
    this.deleted = deleted
    this.connect = connect
    this.limits = limits
    this.random = random
    this.mainId = mainId
    this.client = null
    this.failures = []
    /** conversationId → { ids, seen, cursor, subscription, lost } */
    this.events = new Map()
    this.view = { frame: null, subscription: null, lost: false }
    this.list = { conversations: [], subscription: null, lost: false }
    this.counts = { churned: 0, resumed: 0, lagging: 0, refusedAsExpected: 0 }
    this.stopping = false
    this.loop = null
  }

  fail(what) {
    this.failures.push(`follower ${this.n} at ${new Date().toISOString()}: ${what}`)
  }

  async start() {
    this.client = await this.connect()
    await this.openList()
    await this.openView()
    this.loop = this.run()
  }

  /** The client in use, a new one when the old one gave up reconnecting. */
  async live() {
    if (this.client?.connectionState?.status === "closed") {
      this.client = await this.connect()
    }
    return this.client
  }

  /** `open` until it succeeds; a restart can refuse it for a while. */
  async retry(what, open, conversationId) {
    const deadline = Date.now() + 120_000
    let last
    while (Date.now() < deadline) {
      try {
        return await open(await this.live())
      } catch (error) {
        last = error
        if (this.gone(conversationId)) return null
        // Its own bookkeeping is wrong; waiting does not change that.
        if (REFUSED.has(error?.code)) break
        await sleep(500)
      }
    }
    this.fail(
      `${what} could not be opened again: ${last?.code ?? ""} ${last?.message ?? last}`,
    )
    return null
  }

  /**
   * Whether `conversationId` is one the run deleted: however its stream ends
   * or its follow is refused from then on, it is let go, its events no longer
   * compared, since a deleted conversation has none to compare with.
   */
  gone(conversationId) {
    if (!this.deleted.has(conversationId)) return false
    this.events.delete(conversationId)
    return true
  }

  ended(what, end, lost) {
    if (this.stopping && end.reason === "disconnected") return
    if (end.reason === "lagging") this.counts.lagging += 1
    if (end.reason === "disconnected" || end.reason === "source_closed")
      this.counts.resumed += 1
    if (!RESUMED.has(end.reason)) {
      this.fail(`${what} ended ${end.reason}${end.code ? ` (${end.code})` : ""}`)
      return
    }
    lost()
  }

  async openList() {
    this.list.lost = false
    this.list.subscription = await this.retry("the list", (client) =>
      client.subscriptions.list({
        list: (list) => {
          this.list.conversations = list.conversations.map((each) => each.conversationId)
        },
        ended: (end) => this.ended("the list", end, () => (this.list.lost = true)),
      }),
    )
  }

  async openView() {
    this.view.lost = false
    const after = this.view.frame?.cursor
    this.view.subscription = await this.retry("the main view", (client) =>
      client.subscriptions.view(
        this.mainId,
        {
          view: (frame) => {
            this.view.frame = frame
          },
          ended: (end) => this.ended("the main view", end, () => (this.view.lost = true)),
        },
        after ? { after } : {},
      ),
    )
  }

  /** Follow `conversationId`'s events from the start, or after the last applied cursor. */
  async follow(conversationId) {
    let held = this.events.get(conversationId)
    if (!held) {
      held = { ids: [], seen: new Set(), cursor: null, subscription: null, lost: false }
      this.events.set(conversationId, held)
    }
    held.lost = false
    // Only the stream opened last applies events; a stopped one is ignored.
    const token = {}
    held.token = token
    const subscription = await this.retry(
      `events of ${conversationId}`,
      (client) =>
        client.subscriptions.events(
          conversationId,
          {
            batch: (batch) => {
              if (held.token !== token) return
              for (const event of batch.events) {
                if (held.seen.has(event.id))
                  this.fail(`${conversationId}: event ${event.id} came twice`)
                held.seen.add(event.id)
                held.ids.push(event.id)
              }
              held.cursor = batch.cursor
            },
            ended: (end) => {
              if (held.token !== token) return
              if (this.gone(conversationId)) return
              this.ended(`events of ${conversationId}`, end, () => {
                held.lost = true
                held.subscription = null
              })
            },
          },
          held.cursor ? { after: held.cursor } : { replay: true },
        ),
      conversationId,
    )
    held.subscription = subscription
    return subscription
  }

  /** Stop following `conversationId` for a moment, then follow it again from its cursor. */
  async churn(conversationId) {
    const held = this.events.get(conversationId)
    if (!held) return
    const subscription = held.subscription
    held.subscription = null
    held.token = null
    await subscription?.close().catch(() => {})
    this.counts.churned += 1
    await this.follow(conversationId)
  }

  /**
   * Fill this connection's room with views of other conversations, then ask
   * for one more: that one must be refused, and the ones before it taken.
   */
  async probe() {
    // Only with every stream it keeps room for held, so the room it asks
    // past is the connection's real one.
    if (
      this.view.lost ||
      !this.view.subscription ||
      [...this.events.values()].some((each) => each.lost || !each.subscription)
    )
      return
    const others = this.list.conversations.filter((id) => id !== this.mainId)
    const room = this.limits.conversationTargets - this.held()
    if (room < 0 || others.length < room + 1) return
    const client = this.client
    const originalView = this.view.subscription
    const originals = [...this.events].map(([id, held]) => ({
      id,
      held,
      subscription: held.subscription,
    }))
    const opened = []
    let ended = false
    // Capacity is measured only while the original physical follows and all
    // views acquired by this probe remain live. Their loss frees real room.
    const live = () =>
      !ended &&
      this.client === client &&
      client.connectionState.status === "connected" &&
      !this.view.lost &&
      this.view.subscription === originalView &&
      originals.every(
        ({ id, held, subscription }) =>
          !this.deleted.has(id) &&
          this.events.get(id) === held &&
          !held.lost &&
          held.subscription === subscription,
      )
    try {
      for (let n = 0; n <= room; n += 1) {
        if (!live()) return
        try {
          opened.push(
            await client.subscriptions.view(others[n], {
              view() {},
              ended() {
                ended = true
              },
            }),
          )
          if (!live()) return
          if (n === room)
            this.fail(
              `a subscription past ${this.limits.conversationTargets} was accepted`,
            )
        } catch (error) {
          if (!live()) return
          // The gateway restarting, or this connection dropping, ends the
          // probe: it asks again on a later round.
          if (error?.code === "temporarily_unavailable" || !isRefusal(error)) return
          if (n === room && error?.code === "subscription_capacity")
            this.counts.refusedAsExpected += 1
          else
            this.fail(
              `subscription ${this.held() + n + 1} of ${this.limits.conversationTargets} failed ${error?.code ?? error?.message}`,
            )
          return
        }
      }
    } finally {
      await Promise.all(opened.map((each) => each.close().catch(() => {})))
    }
  }

  /**
   * The room this follower keeps: one for the main view and one for each
   * conversation it follows, held or not. A stream the gateway ended keeps
   * its room until it is followed again, or filling the room in between
   * would leave it nowhere to go.
   */
  held() {
    return this.events.size + 1
  }

  async run() {
    while (!this.stopping) {
      try {
        if (this.list.lost) await this.openList()
        if (this.view.lost) await this.openView()
        for (const [conversationId, held] of this.events)
          if (held.lost && !this.stopping) await this.follow(conversationId)
        // Fill this connection's room with conversations not followed yet.
        for (const id of this.list.conversations) {
          if (this.stopping || this.held() >= this.limits.conversationTargets) break
          if (!this.events.has(id) && !this.deleted.has(id)) await this.follow(id)
        }
        const followed = [...this.events.keys()]
        const roll = this.random()
        if (followed.length && roll < 0.6)
          await this.churn(followed[Math.floor(this.random() * followed.length)])
        else if (roll < 0.7) await this.probe()
      } catch (error) {
        this.fail(`${error?.code ?? ""} ${error?.message ?? error}`)
      }
      await sleep(200 + Math.floor(this.random() * 600))
    }
  }

  /** Stop churning and follow everything again, so the end can be compared. */
  async settle() {
    this.stopping = true
    await this.loop
    this.stopping = false
    if (this.view.lost || !this.view.subscription) await this.openView()
    for (const [conversationId, held] of this.events)
      if (held.lost || !held.subscription) await this.follow(conversationId)
  }

  /**
   * What must hold at the end: each followed conversation's events, as this
   * follower applied them, are its events from the start; the main view is
   * the one `read` gives.
   */
  async check(reader) {
    for (const [conversationId, held] of this.events) {
      if (this.gone(conversationId)) continue
      let expected
      try {
        expected = await replayEvents(reader, conversationId)
      } catch (error) {
        this.fail(error.message)
        continue
      }
      const deadline = Date.now() + 30_000
      while (held.ids.length < expected.length && Date.now() < deadline) await sleep(200)
      const same =
        held.ids.length === expected.length &&
        held.ids.every((id, index) => id === expected[index])
      if (!same) {
        const missing = expected.filter((id) => !held.seen.has(id)).length
        this.fail(
          `${conversationId}: applied ${held.ids.length} events, the conversation has ${expected.length} (${missing} missing)`,
        )
      }
    }
    const turns = (view) =>
      JSON.stringify(view?.messages.map((each) => [each.executionId, each.status]))
    const deadline = Date.now() + 30_000
    let read
    do {
      read = await reader.conversation.read(this.mainId).catch(() => null)
      if (read && turns(read) === turns(this.view.frame?.view)) break
      await sleep(250)
    } while (Date.now() < deadline)
    if (turns(read) !== turns(this.view.frame?.view))
      this.fail(
        `its view of the main conversation is not the one a read gives (read ${read?.transcriptState ?? "failed"} ${turns(read)}; view ${turns(this.view.frame?.view)})`,
      )
    // The list is judged against a list read now, not against the main
    // conversation being in it: the run may have archived that one.
    const ids = (conversations) => JSON.stringify([...conversations].sort())
    const listed = async () =>
      (await reader.conversation.list().catch(() => null))?.conversations.map(
        (each) => each.conversationId,
      )
    const listDeadline = Date.now() + 30_000
    let current
    do {
      current = await listed()
      if (current && ids(current) === ids(this.list.conversations)) break
      await sleep(250)
    } while (Date.now() < listDeadline)
    if (!current || ids(current) !== ids(this.list.conversations))
      this.fail("its list is not the one a list read gives")
    return this.failures
  }

  async close() {
    this.stopping = true
    await this.loop?.catch(() => {})
    this.client?.close()
  }
}
