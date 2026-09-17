import type { AgentReadiness, AgentReadinessReport } from "../model/onboarding"

/**
 * Where the local gateway answers before there is a session.
 *
 * A browser preview reaches it through Vite's proxy on its own origin; the
 * desktop app talks to the gateway directly. Both are the same server.
 */
const AGENTS_URL = import.meta.env.DEV
  ? "/onboarding/agents"
  : "http://127.0.0.1:7420/onboarding/agents"

/** What the gateway says about one agent. Unknown states are not trusted. */
function known(value: unknown): AgentReadiness | undefined {
  return value === "ready" ||
    value === "needs-authentication" ||
    value === "not-installed"
    ? value
    : undefined
}

/**
 * Ask the gateway which agents could actually start here.
 *
 * This is the one thing setup asks before authenticating, because it is asked
 * *while* setting up — there is no session yet and nothing to authenticate
 * with. The gateway answers it unauthenticated for that reason.
 *
 * A gateway that is not running, not reachable, or answering something this
 * build does not understand reports nothing, and nothing reads as "not
 * available": setup would rather offer no agent than offer one that cannot
 * run.
 */
export async function readAgentsReadiness(): Promise<AgentReadinessReport> {
  try {
    const response = await fetch(AGENTS_URL, { headers: { accept: "application/json" } })
    if (!response.ok) return {}
    const body: unknown = await response.json()
    const agents = (body as { agents?: unknown })?.agents
    if (!Array.isArray(agents)) return {}
    const report: Record<string, AgentReadiness> = {}
    for (const entry of agents) {
      const id = (entry as { id?: unknown })?.id
      const readiness = known((entry as { readiness?: unknown })?.readiness)
      if (typeof id === "string" && readiness) report[id] = readiness
    }
    return report
  } catch {
    return {}
  }
}
