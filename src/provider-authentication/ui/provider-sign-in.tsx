import { useEffect, useRef, useState } from "react"
import { AgentActionButton, AgentActionCard } from "./agent-action-card"

/** Provider login is an external flow; launching it does not dismiss the refusal. */
export function ProviderSignInCard({
  provider,
  onSignIn,
  canSignIn,
}: {
  provider: "claude" | "codex"
  onSignIn?: (provider: "claude" | "codex") => Promise<void>
  canSignIn?: () => Promise<boolean>
}) {
  const [capability, setCapability] = useState<{
    check: typeof canSignIn
    action: typeof onSignIn
    supported: boolean
  }>()
  const available =
    !!onSignIn &&
    (!canSignIn ||
      (capability?.check === canSignIn &&
        capability.action === onSignIn &&
        capability.supported))
  useEffect(() => {
    let current = true
    if (canSignIn)
      void canSignIn().then(
        (supported) => {
          if (current) setCapability({ check: canSignIn, action: onSignIn, supported })
        },
        () => {
          if (current)
            setCapability({ check: canSignIn, action: onSignIn, supported: false })
        },
      )
    return () => {
      current = false
    }
  }, [canSignIn, onSignIn])
  const inFlight = useRef(false)
  const [pending, setPending] = useState(false)
  const [failed, setFailed] = useState(false)
  const name = provider === "claude" ? "Claude" : "Codex"
  const signIn = async () => {
    if (!available || inFlight.current) return
    inFlight.current = true
    setPending(true)
    setFailed(false)
    try {
      if (!onSignIn) throw new Error("Provider login unavailable")
      await onSignIn(provider)
    } catch {
      setFailed(true)
    } finally {
      inFlight.current = false
      setPending(false)
    }
  }
  return (
    <AgentActionCard
      className="provider-sign-in"
      label="Your login expired"
      title="Your login expired"
      actions={
        <AgentActionButton
          disabled={pending || !available}
          title={!available ? "Sign-in is unavailable on this host" : undefined}
          onClick={() => void signIn()}
        >
          Sign in to {name}
        </AgentActionButton>
      }
    >
      {failed ? <p role="status">Could not open sign-in. Try again.</p> : null}
    </AgentActionCard>
  )
}
