/**
 * Clients that only follow, for the conversation stress check
 * (`conversation-stress.mjs`). Each holds as many conversation subscriptions
 * as one connection may (`subscriptionLimits`): one view of the main
 * conversation and the events of every other conversation it finds through
 * its list subscription. As it goes it stops one at random and follows it
 * again from the last cursor it applied, asks for one more than it may hold
 * (and must be refused), and follows again after a restart drops its
 * connection or the gateway ends a stream it fell behind on.
 *
 * What it must show at the end: every conversation's events, as it applied
 * them, are exactly the conversation's events from the start (none missed,
 * none twice, none out of order), and its view of the main conversation is
 * the one a read gives.
 */

/** How a subscription may end without anything being wrong. */
const RESUMED = new Set(["disconnected", "lagging"])
/** Refusals that say this follower asked for something it should not have. */
const REFUSED = new Set(["subscription_capacity", "subscription_duplicate"])

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** Each event of `conversationId` from the start, read until nothing comes for `quietMs`. */
export async function replayEvents(client, conversationId, quietMs = 1_500) {
  const ids = []
  let last = Date.now()
  let ended = null
  const subscription = await client.subscriptions.events(
    conversationId,
    {
      batch(batch) {
        for (const event of batch.events) ids.push(event.id)
        last = Date.now()
      },
      ended(end) {
        ended = end
      },
    },
    { replay: true },
  )
  const deadline = Date.now() + 60_000
  while (!ended && Date.now() - last < quietMs && Date.now() < deadline) await sleep(100)
  await subscription.close().catch(() => {})
  if (ended) throw new Error(`a replay of ${conversationId} ended ${ended.reason}`)
  return ids
}

export class Follower {
  /**
   * `connect` makes a new client; `limits` is the published
   * `subscriptionLimits`; `random` the run's seeded generator.
   */
  constructor({ n, connect, limits, random, mainId }) {
    this.n = n
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
  async retry(what, open) {
    const deadline = Date.now() + 120_000
    let last
    while (Date.now() < deadline) {
      try {
        return await open(await this.live())
      } catch (error) {
        last = error
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

  ended(what, end, lost) {
    if (this.stopping && end.reason === "disconnected") return
    if (end.reason === "lagging") this.counts.lagging += 1
    if (end.reason === "disconnected") this.counts.resumed += 1
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
    const subscription = await this.retry(`events of ${conversationId}`, (client) =>
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
            this.ended(`events of ${conversationId}`, end, () => {
              held.lost = true
              held.subscription = null
            })
          },
        },
        held.cursor ? { after: held.cursor } : { replay: true },
      ),
    )
    held.subscription = subscription
    return subscription
  }

  /** Stop following `conversationId` for a moment, then follow it again from its cursor. */
  async churn(conversationId) {
    const held = this.events.get(conversationId)
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
    if (this.view.lost || [...this.events.values()].some((each) => !each.subscription)) return
    const others = this.list.conversations.filter((id) => id !== this.mainId)
    const room = this.limits.conversationTargets - this.held()
    if (room < 0 || others.length < room + 1) return
    const opened = []
    try {
      for (let n = 0; n <= room; n += 1) {
        try {
          opened.push(
            await this.client.subscriptions.view(others[n], { view() {}, ended() {} }),
          )
          if (n === room)
            this.fail(`a subscription past ${this.limits.conversationTargets} was accepted`)
        } catch (error) {
          if (error?.code === "temporarily_unavailable") return
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
          if (!this.events.has(id)) await this.follow(id)
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
      this.fail("its view of the main conversation is not the one a read gives")
    if (!this.list.conversations.includes(this.mainId))
      this.fail("its list does not hold the main conversation")
    return this.failures
  }

  async close() {
    this.stopping = true
    await this.loop?.catch(() => {})
    this.client?.close()
  }
}
