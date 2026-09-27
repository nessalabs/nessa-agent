import { describe, expect, it, vi } from "vitest"
import { httpAgentReadiness } from "./agents"

function answering(body: unknown, init: { status?: number } = {}) {
  return vi.fn(async () =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status: init.status ?? 200,
        headers: { "content-type": "application/json" },
      }),
    ),
  )
}

const READY = { agents: [{ id: "claude", readiness: "ready" }] }

describe("asking the gateway which agents can start", () => {
  it("asks where the environment says the gateway answers", async () => {
    const fetch = answering(READY)
    await httpAgentReadiness({ baseUrl: "http://127.0.0.1:7420", fetch }).read()
    expect(fetch).toHaveBeenCalledWith("http://127.0.0.1:7420/onboarding/agents", {
      headers: { accept: "application/json" },
    })
  })

  it("asks this page's own origin when the base is empty", async () => {
    // A development server proxies the gateway onto it, so there is no port to
    // name and nothing here has to know one.
    const fetch = answering(READY)
    await httpAgentReadiness({ baseUrl: "", fetch }).read()
    expect(fetch).toHaveBeenCalledWith("/onboarding/agents", expect.anything())
  })

  it("reports what each runtime said", async () => {
    const fetch = answering({
      agents: [
        { id: "claude", readiness: "ready" },
        { id: "codex", readiness: "not-installed" },
      ],
    })
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: true,
      agents: { claude: "ready", codex: "not-installed" },
    })
  })

  it("carries an agent this gateway was not set up for as its own answer", async () => {
    // Not folded into "not installed". The agent may be sitting on the machine
    // already, and telling someone to install it is advice that cannot work
    // however many times they take it.
    const fetch = answering({
      agents: [
        { id: "claude", readiness: "ready" },
        { id: "codex", readiness: "not-configured" },
      ],
    })
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: true,
      agents: { claude: "ready", codex: "not-configured" },
    })
  })

  it("drops an entry whose readiness this build does not understand", async () => {
    // A newer gateway naming a state this build cannot act on is not a reason
    // to discard the states it can.
    const fetch = answering({
      agents: [
        { id: "claude", readiness: "ready" },
        { id: "codex", readiness: "quantum-entangled" },
        { readiness: "ready" },
        "nonsense",
      ],
    })
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: true,
      agents: { claude: "ready" },
    })
  })

  /**
   * The report is keyed by agent, and the gateway names the agents. Any string
   * it sent used to become a key: `constructor` wrote an own property over the
   * one every object already has, and `__proto__` went to the prototype setter
   * and was not stored at all — so the report neither held what arrived nor
   * said it had not. An id is a key only if it is an agent Nessa lists.
   */
  it("keys the report by the agents Nessa lists, not by whatever was sent", async () => {
    const fetch = answering({
      agents: [
        { id: "claude", readiness: "ready" },
        { id: "constructor", readiness: "ready" },
        { id: "__proto__", readiness: "ready" },
        { id: "toString", readiness: "not-installed" },
        { id: "gemini", readiness: "ready" },
      ],
    })
    const answer = await httpAgentReadiness({ baseUrl: "", fetch }).read()
    expect(answer).toEqual({ ok: true, agents: { claude: "ready" } })
    expect(Object.keys(answer.ok ? answer.agents : {})).toEqual(["claude"])
  })

  it("is an answer, not a failure, when no runtime can start anything", async () => {
    const fetch = answering({ agents: [] })
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: true,
      agents: {},
    })
  })

  it("reports a gateway that is not there as unreachable", async () => {
    // The failure the person can actually fix. Reporting it as "this agent is
    // unavailable" blamed Claude for Nessa's own server not running.
    const fetch = vi.fn(() => Promise.reject(new TypeError("Failed to fetch")))
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: false,
      reason: "unreachable",
    })
  })

  it.each([404, 500, 503])("reports a %d as unreachable", async (status) => {
    const fetch = answering({ agents: [] }, { status })
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: false,
      reason: "unreachable",
    })
  })

  it("reports a body that is not JSON as unreadable", async () => {
    const fetch = vi.fn(async () =>
      Promise.resolve(new Response("<html>gateway is confused</html>", { status: 200 })),
    )
    await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
      ok: false,
      reason: "unreadable",
    })
  })

  it.each([{ agents: "claude" }, { agents: null }, {}, [], 7])(
    "reports %j as unreadable",
    async (body) => {
      const fetch = answering(body)
      await expect(httpAgentReadiness({ baseUrl: "", fetch }).read()).resolves.toEqual({
        ok: false,
        reason: "unreadable",
      })
    },
  )

  it("never rejects, whatever the transport does", async () => {
    const fetch = vi.fn(() => {
      throw new Error("synchronous explosion")
    })
    await expect(
      httpAgentReadiness({ baseUrl: "", fetch }).read(),
    ).resolves.toHaveProperty("ok", false)
  })
})
