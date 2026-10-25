/**
 * The pickers' catalog is followed, not read once: each catalog the gateway
 * sends replaces the last, a follow that ends is opened again, and a stopped
 * follow gives its subscription up and shows nothing more.
 */
import { expect, it, vi } from "vitest"
import type { AgentsListResult, NessaClient } from "@nessa/client"
import { followAgentChoices } from "./agent-choices"

type Handlers = {
  catalog(catalog: AgentsListResult): void
  ended(end: { reason: string }): void
}

/** A connected client whose agent catalog subscriptions the test drives. */
function gateway() {
  const opens: { handlers: Handlers; signal: AbortSignal }[] = []
  const agents = vi.fn(async (handlers: Handlers, options: { signal: AbortSignal }) => {
    opens.push({ handlers, signal: options.signal })
    return { id: String(opens.length), close: async () => undefined }
  })
  const client = {
    connectionState: { status: "connected" },
    subscriptions: { agents },
  } as unknown as NessaClient
  return { client, opens, agents }
}

/** A retry the test lets go by hand. */
function retries() {
  const waiting: (() => void)[] = []
  return {
    retry: () => new Promise<void>((resolve) => waiting.push(resolve)),
    waiting,
  }
}

const catalog = (...environments: string[]): AgentsListResult => ({
  agents: [],
  environments,
  commandHosts: [],
  nativeHooks: { claude: "off", codex: "off" },
})

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

it("shows each catalog the gateway sends, with the agent setup chose", async () => {
  const { client, opens } = gateway()
  const show = vi.fn()
  const stop = followAgentChoices(
    () => client,
    retries().retry,
    async () => "codex",
  )(show)
  await settle()
  opens[0].handlers.catalog(catalog("devbox"))
  await settle()
  expect(show).toHaveBeenLastCalledWith({
    catalog: catalog("devbox"),
    chosenAgent: "codex",
  })
  const peer = `peer:${"ab".repeat(32)}`
  opens[0].handlers.catalog(catalog("devbox", peer))
  await settle()
  expect(show).toHaveBeenLastCalledWith({
    catalog: catalog("devbox", peer),
    chosenAgent: "codex",
  })
  expect(show).toHaveBeenCalledTimes(2)
  stop()
})

it("never shows an older catalog over a newer one whose agent was answered first", async () => {
  const { client, opens } = gateway()
  const answers: ((agent: string | undefined) => void)[] = []
  const show = vi.fn()
  const stop = followAgentChoices(
    () => client,
    retries().retry,
    () => new Promise((resolve) => answers.push(resolve)),
  )(show)
  await settle()
  opens[0].handlers.catalog(catalog("old"))
  opens[0].handlers.catalog(catalog("new"))
  answers[1](undefined)
  answers[0](undefined)
  await settle()
  expect(show).toHaveBeenCalledExactlyOnceWith({
    catalog: catalog("new"),
    chosenAgent: undefined,
  })
  stop()
})

it("gives its subscription up when stopped and shows nothing it says after", async () => {
  const { client, opens } = gateway()
  const show = vi.fn()
  const stop = followAgentChoices(
    () => client,
    retries().retry,
    async () => undefined,
  )(show)
  await settle()
  stop()
  expect(opens[0].signal.aborted).toBe(true)
  opens[0].handlers.catalog(catalog("devbox"))
  await settle()
  expect(show).not.toHaveBeenCalled()
})

it("opens again at once after lagging, and on the retry after any other end", async () => {
  const { client, opens, agents } = gateway()
  const { retry, waiting } = retries()
  const show = vi.fn()
  const stop = followAgentChoices(
    () => client,
    retry,
    async () => undefined,
  )(show)
  await settle()
  opens[0].handlers.ended({ reason: "lagging" })
  await settle()
  expect(agents).toHaveBeenCalledTimes(2)
  expect(waiting).toHaveLength(0)
  opens[1].handlers.ended({ reason: "disconnected" })
  await settle()
  expect(agents).toHaveBeenCalledTimes(2)
  expect(waiting).toHaveLength(1)
  waiting[0]()
  await settle()
  expect(agents).toHaveBeenCalledTimes(3)
  // An earlier open's frames apply nothing.
  opens[1].handlers.catalog(catalog("stale"))
  opens[2].handlers.catalog(catalog("fresh"))
  await settle()
  expect(show).toHaveBeenCalledExactlyOnceWith({
    catalog: catalog("fresh"),
    chosenAgent: undefined,
  })
  stop()
})

it("waits for the retry when there is no gateway to subscribe to", async () => {
  const { client, agents } = gateway()
  const { retry, waiting } = retries()
  let current: NessaClient | null = null
  const stop = followAgentChoices(
    () => current,
    retry,
    async () => undefined,
  )(vi.fn())
  await settle()
  expect(waiting).toHaveLength(1)
  current = client
  waiting[0]()
  await settle()
  expect(agents).toHaveBeenCalledOnce()
  stop()
})

it("shows a choice setup recorded against the catalog already shown", async () => {
  const { client, opens } = gateway()
  const show = vi.fn()
  let agent = "claude"
  let recorded: (() => void) | undefined
  const unlisten = vi.fn()
  const stop = followAgentChoices(
    () => client,
    retries().retry,
    async () => agent,
    (listener) => {
      recorded = listener
      return unlisten
    },
  )(show)
  // Nothing shown yet: there is no catalog to show the choice against.
  recorded?.()
  await settle()
  expect(show).not.toHaveBeenCalled()
  opens[0].handlers.catalog(catalog("devbox"))
  await settle()
  expect(show).toHaveBeenLastCalledWith({
    catalog: catalog("devbox"),
    chosenAgent: "claude",
  })
  // A replay finished on another agent, and the gateway sent no new catalog.
  agent = "codex"
  recorded?.()
  await settle()
  expect(show).toHaveBeenLastCalledWith({
    catalog: catalog("devbox"),
    chosenAgent: "codex",
  })
  stop()
  expect(unlisten).toHaveBeenCalledTimes(1)
  recorded?.()
  await settle()
  expect(show).toHaveBeenCalledTimes(2)
})
