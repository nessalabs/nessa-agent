import * as React from "react"
import type {
  AgentInstallations,
  InstallableAgent,
  InstallationOffer,
  InstallationOutcome,
} from "../application/agent-installations"

/** Local presentation of server-owned installation. No automatic install or replay. */
export function useAgentDownloads(source: AgentInstallations, onInstalled?: () => void) {
  const [offers, setOffers] = React.useState<readonly InstallationOffer[]>()
  const [pending, setPending] = React.useState<InstallableAgent>()
  const [outcome, setOutcome] = React.useState<InstallationOutcome>()
  const [unavailable, setUnavailable] = React.useState(false)
  const active = React.useRef(true)
  const working = React.useRef(false)
  const revision = React.useRef(0)
  const lifetime = React.useRef(0)
  const refresh = React.useCallback(async () => {
    const request = ++revision.current
    try {
      const next = await source.offers()
      if (!active.current || revision.current !== request) return
      setOffers(next)
      setUnavailable(false)
    } catch {
      if (active.current && revision.current === request) setUnavailable(true)
    }
  }, [source])
  React.useEffect(() => {
    active.current = true
    lifetime.current += 1
    working.current = false
    setPending(undefined)
    setOutcome(undefined)
    setOffers(undefined)
    void refresh()
    const unsubscribe = source.subscribe(() => void refresh())
    return () => {
      unsubscribe()
      active.current = false
      lifetime.current += 1
      revision.current += 1
    }
  }, [refresh, source])
  const install = async (agent: InstallableAgent) => {
    if (working.current) return
    const generation = lifetime.current
    working.current = true
    setPending(agent)
    setOutcome(undefined)
    let result: InstallationOutcome
    try {
      result = await source.install(agent)
    } catch {
      result = { status: "failed", reason: "not-confirmed" }
    }
    if (!active.current || generation !== lifetime.current) return
    working.current = false
    setPending(undefined)
    setOutcome(result)
    await refresh()
    if (active.current && generation === lifetime.current) onInstalled?.()
  }
  return {
    offers,
    pending,
    outcome,
    unavailable,
    refresh: () => void refresh(),
    install: (agent: InstallableAgent) => void install(agent),
  }
}
