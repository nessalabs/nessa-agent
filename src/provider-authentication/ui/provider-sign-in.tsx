import { useEffect, useRef, useState } from "react"
import { BoatSignInFailure, type SignInAction } from "../model/boat-sign-in"
import { AgentActionButton, AgentActionCard } from "./agent-action-card"

/**
 * Provider login is an external flow; launching it does not dismiss the
 * refusal. `boat` mints a sign-in for Boat VMs instead (ADR 501): it resolves
 * once the gateway kept it, and the person's next message runs with it.
 */
export function ProviderSignInCard({
  provider,
  onSignIn,
  canSignIn,
  action = "computer",
}: {
  provider: "claude" | "codex"
  onSignIn?: (provider: "claude" | "codex") => Promise<void>
  canSignIn?: () => Promise<boolean>
  action?: SignInAction
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
  const [failed, setFailed] = useState<"opened" | "minted" | "saved">()
  const [signedIn, setSignedIn] = useState(false)
  const name = provider === "claude" ? "Claude" : "Codex"
  const signIn = async () => {
    if (!available || inFlight.current) return
    inFlight.current = true
    setPending(true)
    setFailed(undefined)
    try {
      if (!onSignIn) throw new Error("Provider login unavailable")
      await onSignIn(provider)
      if (action === "boat") setSignedIn(true)
    } catch (error) {
      setFailed(error instanceof BoatSignInFailure ? error.stage : "opened")
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
        signedIn ? null : (
          <AgentActionButton
            disabled={pending || !available}
            title={!available ? "Sign-in is unavailable on this host" : undefined}
            onClick={() => void signIn()}
          >
            {action === "boat" ? "Authenticate" : `Sign in to ${name}`}
          </AgentActionButton>
        )
      }
    >
      {signedIn ? (
        <p role="status">Signed in for Boat workspaces. Send your message again.</p>
      ) : pending && action === "boat" ? (
        <p role="status">Finish signing in in your browser.</p>
      ) : failed ? (
        <p role="status">{failureText[failed]}</p>
      ) : null}
    </AgentActionCard>
  )
}

const failureText = {
  opened: "Could not open sign-in. Try again.",
  minted: "Could not sign in. Try again.",
  saved: "Could not save the sign-in. Unlock Nessa and try again.",
} as const
