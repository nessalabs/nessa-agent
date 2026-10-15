import { expect, it, vi } from "vitest"
import { assignedAgentReadiness } from "./agents"

it("reads readiness at the verified assigned nondefault endpoint", async () => {
  const fetch = vi.fn(
    async () =>
      ({
        ok: true,
        json: async () => ({ agents: [{ id: "claude", readiness: "not-installed" }] }),
      }) as Response,
  )
  const source = assignedAgentReadiness({
    endpoint: async () => "ws://127.0.0.1:17999/session",
    fetch,
  })
  expect(await source.read()).toEqual({ ok: true, agents: { claude: "not-installed" } })
  expect(fetch).toHaveBeenCalledWith(
    "http://127.0.0.1:17999/onboarding/agents",
    expect.anything(),
  )
})

it("bounds assigned-endpoint lookup by the same overall readiness deadline", async () => {
  let expire!: () => void
  let publish!: (value: string | undefined) => void
  const endpoint = new Promise<string | undefined>((resolve) => {
    publish = resolve
  })
  const fetch = vi.fn()
  const cancel = vi.fn()
  const source = assignedAgentReadiness({
    endpoint: () => endpoint,
    fetch,
    scheduleDeadline: (elapsed) => {
      expire = elapsed
      return cancel
    },
  })
  const answer = source.read()
  expire()
  expect(await answer).toEqual({ ok: false, reason: "unreachable" })
  expect(cancel).toHaveBeenCalledTimes(1)
  publish("ws://127.0.0.1:17999")
  await new Promise<void>((resolve) => setImmediate(resolve))
  expect(fetch).not.toHaveBeenCalled()
})

it("does not guess a stage port when the host assignment is absent or refused", async () => {
  const fetch = vi.fn()
  const absent = assignedAgentReadiness({ endpoint: async () => undefined, fetch })
  expect(await absent.read()).toEqual({ ok: false, reason: "unreachable" })
  const refused = assignedAgentReadiness({
    endpoint: async () => {
      throw new Error("not available")
    },
    fetch,
  })
  expect(await refused.read()).toEqual({ ok: false, reason: "unreachable" })
  expect(fetch).not.toHaveBeenCalled()
})
