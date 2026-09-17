import type { AgentReadiness, AgentReadinessReport } from "../model/onboarding"
import type { AgentReadinessAnswer, AgentReadinessSource } from "../application/ports"

/** The gateway's pre-authentication surface, relative to wherever it answers. */
const AGENTS_PATH = "/onboarding/agents"

/** What the gateway says about one agent. Unknown states are not trusted.
 *
 * `not-supported` and `unknown` are deliberately not accepted here: the first
 * is a fact about Nessa's own listing and the second is the absence of an
 * answer. Neither is something a runtime gets to assert. */
function known(value: unknown): AgentReadiness | undefined {
  return value === "ready" ||
    value === "needs-authentication" ||
    value === "not-installed"
    ? value
    : undefined
}

function readReport(body: unknown): AgentReadinessReport | undefined {
  const agents = (body as { agents?: unknown })?.agents
  if (!Array.isArray(agents)) return undefined
  const report: Record<string, AgentReadiness> = {}
  for (const entry of agents) {
    const id = (entry as { id?: unknown })?.id
    const readiness = known((entry as { readiness?: unknown })?.readiness)
    if (typeof id === "string" && readiness) report[id] = readiness
  }
  return report
}

/**
 * Ask the gateway which agents could actually start here, over HTTP.
 *
 * This is the one thing setup asks before authenticating, because it is asked
 * *while* setting up — there is no session yet and nothing to authenticate
 * with. The gateway answers it unauthenticated for that reason.
 *
 * Where it answers is configuration, not a constant: `baseUrl` comes from the
 * environment seam so this file never reads `import.meta.env` and never has to
 * know a port number. An empty base means this page's own origin, which is how
 * a browser preview reaches the gateway through the dev server's proxy.
 *
 * Not getting an answer is reported as not getting an answer. A gateway that is
 * not running is a thing the person can fix, and saying "not available" about
 * the agent instead hid that from them.
 */
export function httpAgentReadiness({
  baseUrl,
  fetch = globalThis.fetch,
}: {
  /** Where the gateway answers, with no trailing slash. "" is this origin. */
  baseUrl: string
  /** Injected for tests; the page's own `fetch` otherwise. */
  fetch?: typeof globalThis.fetch
}): AgentReadinessSource {
  const url = `${baseUrl}${AGENTS_PATH}`
  return {
    async read(): Promise<AgentReadinessAnswer> {
      let response: Response
      try {
        response = await fetch(url, { headers: { accept: "application/json" } })
      } catch {
        return { ok: false, reason: "unreachable" }
      }
      // A refusal is as good as silence here: the gateway is up but has no
      // answer to give, and there is nothing for setup to tell apart.
      if (!response.ok) return { ok: false, reason: "unreachable" }
      let body: unknown
      try {
        body = await response.json()
      } catch {
        return { ok: false, reason: "unreadable" }
      }
      const agents = readReport(body)
      if (!agents) return { ok: false, reason: "unreadable" }
      return { ok: true, agents }
    },
  }
}
