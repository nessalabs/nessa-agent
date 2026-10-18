/**
 * One process of the scale check's clients (`gateway-scale.mjs` forks a few,
 * so no single event loop is what the run measures). It is told, over IPC,
 * how many sockets to open and how many conversations each one owns, and
 * then runs the phases it is sent one at a time:
 *
 *   connect   opens its sockets (each a `NessaClient`, signed in as the owner)
 *   create    creates its conversations on the scale agent, `parallel` at once
 *   turn      sends each conversation one message and waits for its
 *             `turn.ended` event, `parallel` at once
 *   list      reads `conversation.list` once per socket
 *   close     closes every socket
 *
 * Each phase answers with its latencies (ms, one per operation) and its
 * failures counted by error code, so the parent can tell a refusal the
 * gateway chose (`temporarily_unavailable`, `capacity`) from a crash.
 */
import { randomUUID } from "node:crypto"
import { WebSocket } from "ws"

globalThis.WebSocket = WebSocket
const { register } = await import("tsx/esm/api")
register()
const { NessaClient, NessaClientConfig } = await import("@nessa/client")

let setup
const clients = []
/** Conversation ids by socket index. */
const owned = []

const code = (error) =>
  String(error?.code ?? error?.cause?.code ?? error?.name ?? "error").slice(0, 64)

/** Runs `work(i)` for i in [0, count), `parallel` at once; latencies and failures. */
async function pool(count, parallel, work) {
  const latencies = []
  const failures = {}
  const samples = {}
  let next = 0
  const lane = async () => {
    for (;;) {
      const i = next++
      if (i >= count) return
      const started = performance.now()
      try {
        await work(i)
        latencies.push(performance.now() - started)
      } catch (error) {
        const key = code(error)
        failures[key] = (failures[key] ?? 0) + 1
        samples[key] ??= String(error?.message ?? error).slice(0, 300)
      }
    }
  }
  await Promise.all(Array.from({ length: Math.max(1, parallel) }, lane))
  return { latencies, failures, samples }
}

/**
 * Every conversation as [socket index, conversation id], taking one from each
 * socket in turn, so the operations in flight at once are spread over the
 * sockets rather than queued on one.
 */
const conversations = () => {
  const longest = Math.max(0, ...owned.map((ids) => ids?.length ?? 0))
  const all = []
  for (let n = 0; n < longest; n++)
    owned.forEach((ids, socket) => {
      if (ids && n < ids.length) all.push([socket, ids[n]])
    })
  return all
}

const phases = {
  async connect() {
    return pool(setup.sockets, setup.parallel, async (i) => {
      clients[i] = await NessaClient.connect({
        stage: "ci",
        url: setup.url,
        role: "surface",
        surface: { kind: "panel", instance: setup.instance },
        client: { id: `scale-${setup.worker}-${i}`, version: "0.1.0", platform: "node" },
        profile: "product",
        auth: { credential: setup.credential },
        config: new NessaClientConfig({
          reconnect: { enabled: false },
          requestTimeoutMs: setup.requestTimeoutMs,
        }),
      })
      owned[i] = []
    })
  },

  async create() {
    // Round robin over the open sockets, `setup.limit` in all.
    const open = clients.flatMap((client, socket) => (client ? [socket] : []))
    const wanted = open.length
      ? Array.from({ length: setup.limit }, (_, i) => open[i % open.length])
      : []
    return pool(wanted.length, setup.parallel, async (i) => {
      const socket = wanted[i]
      const conversationId = randomUUID()
      await clients[socket].createThread({ conversationId, agent: setup.agent })
      owned[socket].push(conversationId)
    })
  },

  // Each step's time is kept too, so a slow turn says whether it waited on
  // its subscription, its send, or the turn itself.
  async turn({ text }) {
    const all = conversations()
    const steps = { subscribe: [], send: [], end: [] }
    const result = await pool(all.length, setup.parallel, async (i) => {
      const [socket, id] = all[i]
      const client = clients[socket]
      let at = performance.now()
      const lap = (step) => {
        const now = performance.now()
        steps[step].push(now - at)
        at = now
      }
      const stream = await client.subscribe(id, { kinds: ["turn.ended"] })
      lap("subscribe")
      try {
        const sent = await client.sendMessage(id, `${text} ${i}`)
        lap("send")
        const executionId = sent.executionId
        const deadline = setTimeout(() => stream.close(), setup.turnTimeoutMs)
        try {
          for await (const batch of stream) {
            for (const event of batch.events ?? []) {
              if (event.kind !== "turn.ended") continue
              if (executionId && event.executionId && event.executionId !== executionId)
                continue
              if (event.outcome && event.outcome !== "completed")
                throw Object.assign(new Error(`the turn ended ${event.outcome}`), {
                  code: `turn_${event.outcome}`,
                })
              lap("end")
              return
            }
          }
          throw Object.assign(new Error("no turn.ended before the deadline"), {
            code: "turn_timeout",
          })
        } finally {
          clearTimeout(deadline)
        }
      } finally {
        await stream.close().catch(() => {})
      }
    })
    return { ...result, steps }
  },

  // As the desktop's index reads it: the list, then `observe` pages until
  // the catalogue says it is whole.
  async list() {
    const live = clients.flatMap((client) => (client ? [client] : []))
    // The fewest any socket saw: every socket must see them all.
    let listed = Infinity
    const result = await pool(live.length, setup.parallel, async (i) => {
      const api = live[i].conversation
      // Distinct ids, so a page that repeats one conversation and leaves
      // out another does not add up to the right count.
      const seen = new Set()
      const take = (conversations) => {
        for (const conversation of conversations) seen.add(conversation.conversationId)
      }
      const first = await api.list()
      if (first.complete) take(first.conversations)
      else {
        let cursor
        for (;;) {
          const page = await api.observe(cursor ? { cursor } : {})
          take(page.conversations)
          if (page.complete || !page.cursor) break
          cursor = page.cursor
        }
      }
      listed = Math.min(listed, seen.size)
    })
    return { ...result, listed: Number.isFinite(listed) ? listed : 0 }
  },

  async close() {
    for (const client of clients) client?.close()
    return { latencies: [], failures: {}, samples: {} }
  },
}

process.on("message", async (message) => {
  if (message.type === "setup") {
    setup = message.setup
    process.send({ type: "ready" })
    return
  }
  const phase = phases[message.type]
  if (!phase) return
  try {
    process.send({ type: "done", phase: message.type, result: await phase(message) })
  } catch (error) {
    process.send({
      type: "done",
      phase: message.type,
      error: String(error?.stack ?? error),
    })
  }
})
