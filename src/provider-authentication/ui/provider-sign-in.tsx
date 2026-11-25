import { useEffect, useRef, useState } from "react"
import type { ChatgptSignInEnd } from "@nessa/client"
import { chatgptEndSentence, type CodexChatgptSignIn } from "../model/chatgpt-sign-in"
import { AgentActionButton, AgentActionCard } from "./agent-action-card"

/**
 * Provider login is an external flow; launching it does not dismiss the
 * refusal.
 */
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
          {`Sign in to ${name}`}
        </AgentActionButton>
      }
    >
      {failed ? <p role="status">Could not open sign-in. Try again.</p> : null}
    </AgentActionCard>
  )
}

/**
 * Codex on another computer signs in with a ChatGPT plan: the gateway keeps
 * the account and hands that computer its tokens. The button starts the
 * sign-in and opens the browser on it where the host can; the card links
 * to it as well, and says how it ended. Signed in, the person's next message
 * runs with it.
 */
export function ChatgptSignInCard({ signIn }: { signIn?: CodexChatgptSignIn }) {
  const [available, setAvailable] = useState<{ for: typeof signIn; yes: boolean }>()
  useEffect(() => {
    let current = true
    if (signIn)
      void signIn.available().then((yes) => {
        if (current) setAvailable({ for: signIn, yes })
      })
    return () => {
      current = false
    }
  }, [signIn])
  const offered = !!signIn && available?.for === signIn && available.yes
  const inFlight = useRef(false)
  const [waiting, setWaiting] = useState<string>()
  const [pending, setPending] = useState(false)
  const [end, setEnd] = useState<ChatgptSignInEnd | "failed">()
  const start = async () => {
    if (!signIn || !offered || inFlight.current) return
    inFlight.current = true
    setPending(true)
    setEnd(undefined)
    try {
      setEnd(await signIn.signIn(setWaiting))
    } catch {
      setEnd("failed")
    } finally {
      inFlight.current = false
      setPending(false)
      setWaiting(undefined)
    }
  }
  const signedIn = end === "signed_in"
  const said =
    end === "failed"
      ? "Could not sign in with ChatGPT. Try again."
      : end && !signedIn
        ? chatgptEndSentence(end)
        : undefined
  return (
    <AgentActionCard
      className="provider-sign-in"
      label="Codex needs a sign-in"
      title="Codex needs a sign-in"
      actions={
        signedIn ? null : (
          <AgentActionButton
            disabled={pending || !offered}
            title={
              !offered ? "ChatGPT sign-in is unavailable on this gateway" : undefined
            }
            onClick={() => void start()}
            data-chatgpt-card-continue
          >
            Continue with ChatGPT
          </AgentActionButton>
        )
      }
    >
      {signedIn ? (
        <p role="status">Signed in with ChatGPT. Send your message again.</p>
      ) : waiting ? (
        <p role="status">
          Finish signing in to ChatGPT in your browser.{" "}
          <a href={waiting} data-chatgpt-card-open>
            Open ChatGPT
          </a>
        </p>
      ) : said ? (
        <p role="status">{said}</p>
      ) : (
        <p>Codex runs on another computer here. Use your ChatGPT plan for it.</p>
      )}
    </AgentActionCard>
  )
}
