import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { PairingCode } from "@nessa-ui/react/pairing-code"
import type { GithubGateway } from "../adapters/github-gateway"
import type { GithubOutcome, GithubView } from "../model/github"
import { GithubLiveHelp } from "./github-live-help"
import { setting } from "../model/settings-catalogue"
import { FoundSetting, ItemRow, RowAction, RowLink, Toggle } from "./settings-controls"

/**
 * Settings › Connections › Accounts › GitHub: the gateway's one GitHub
 * sign-in, through the Nessa GitHub App's device flow. Connect shows the code
 * to enter on GitHub; the gateway waits for it, so the row only reads again
 * every `pollMs` while a code is shown, and when the window regains focus or
 * becomes visible. Signed in, the row names the account and links to
 * choosing which repositories the app may reach. With no gateway it rests.
 *
 * ```text
 *   signed out ──Connect──▶ connecting ──entered on GitHub──▶ connected
 *        ▲                     │  (read every pollMs)              │
 *        └──Cancel, expiry, ───┘                                   │
 *        └────────────────────────────Disconnect───────────────────┘
 * ```
 */
const GithubContext = createContext<GithubGateway | undefined>(undefined)

export function GithubProvider({
  gateway,
  children,
}: {
  gateway: GithubGateway | undefined
  children: ReactNode
}) {
  return <GithubContext.Provider value={gateway}>{children}</GithubContext.Provider>
}

type Phase =
  | { readonly kind: "reading" }
  | { readonly kind: "unread"; readonly sentence: string }
  | {
      readonly kind: "ready"
      readonly view: GithubView
      /** An action is in flight: every button rests until it answers. */
      readonly busy?: boolean
      /** Why the last action failed, until the next one. */
      readonly failed?: string
      /** Why the last read failed, until one answers. */
      readonly unread?: string
    }

type Request = { readonly kind: "read" } | { readonly kind: "act" }

/** What disconnecting does not do, said where the button is. */

export function GithubRow() {
  const gateway = useContext(GithubContext)
  const found = useContext(FoundSetting) === "github"
  const entry = setting("github")
  const [phase, setPhase] = useState<Phase>({ kind: "reading" })
  // An action owns admission until it answers; a read that started before it
  // is dropped, so an older answer never replaces a newer one.
  const latest = useRef<Request>({ kind: "read" })
  // One read at a time: a poll or focus while one is out waits for its
  // answer rather than overtaking it, so a gateway slower than the poll
  // still has its answers shown.
  const reading = useRef(false)
  const read = useRef<() => void>(() => undefined)
  useEffect(() => {
    if (!gateway) return
    let current = true
    read.current = () => {
      if (latest.current.kind === "act" || reading.current) return
      const request: Request = { kind: "read" }
      latest.current = request
      reading.current = true
      void gateway.read().then((outcome) => {
        reading.current = false
        if (!current || request !== latest.current) return
        setPhase((previous) => settled(previous, outcome))
      })
    }
    setPhase({ kind: "reading" })
    read.current()
    const onFocus = () => read.current()
    const onVisible = () => {
      if (document.visibilityState === "visible") read.current()
    }
    window.addEventListener("focus", onFocus)
    document.addEventListener("visibilitychange", onVisible)
    return () => {
      current = false
      reading.current = false
      latest.current = { kind: "read" }
      read.current = () => undefined
      window.removeEventListener("focus", onFocus)
      document.removeEventListener("visibilitychange", onVisible)
    }
  }, [gateway])
  // While a code is shown, the gateway is waiting on GitHub: read again on
  // the gateway's pace, and stop the moment the row leaves that state.
  const waiting = phase.kind === "ready" && phase.view.kind === "connecting"
  useEffect(() => {
    if (!gateway || !waiting) return
    const timer = window.setInterval(() => read.current(), gateway.pollMs)
    return () => window.clearInterval(timer)
  }, [gateway, waiting])
  // A code's deadline is fixed when it is first seen, so later reads do not
  // nudge the countdown.
  const deadline = useRef<{ code: string; at: number } | null>(null)
  if (phase.kind === "ready" && phase.view.kind === "connecting") {
    const { userCode, expiresInSeconds } = phase.view
    if (deadline.current?.code !== userCode)
      deadline.current = { code: userCode, at: Date.now() + expiresInSeconds * 1_000 }
  }

  const act = (action: (gateway: GithubGateway) => Promise<GithubOutcome>) => {
    if (!gateway || phase.kind !== "ready" || latest.current.kind === "act") return
    const request: Request = { kind: "act" }
    latest.current = request
    const view = phase.view
    setPhase({ kind: "ready", view, busy: true })
    void action(gateway).then(async (outcome) => {
      if (request !== latest.current) return
      if (outcome.ok) {
        latest.current = { kind: "read" }
        setPhase({ kind: "ready", view: outcome.view })
        return
      }
      // A lost answer may still have been applied: what the gateway now says is in effect.
      const again = await gateway.read()
      if (request !== latest.current) return
      latest.current = { kind: "read" }
      setPhase(
        again.ok
          ? { kind: "ready", view: again.view, failed: outcome.sentence }
          : { kind: "ready", view, failed: outcome.sentence },
      )
    })
  }

  if (!gateway || phase.kind === "reading")
    return (
      <ItemRow
        setting="github"
        found={found}
        label={entry.label}
        detail={gateway ? "Reading the sign-in." : "Not available in this window."}
        unavailable
        data-github-state={gateway ? "reading" : "absent"}
      />
    )
  if (phase.kind === "unread")
    return (
      <ItemRow
        setting="github"
        found={found}
        label={entry.label}
        detail={phase.sentence}
        data-github-state="unread"
        control={<RowAction onClick={() => read.current()}>Try again</RowAction>}
      />
    )
  const { view, busy } = phase
  const failed = phase.unread ?? phase.failed
  switch (view.kind) {
    case "signedOut":
      return (
        <ItemRow
          setting="github"
          found={found}
          label={entry.label}
          detail={failed ?? view.ended ?? entry.detail}
          data-github-state="signed-out"
          control={
            <RowAction disabled={busy} onClick={() => act((each) => each.connect())}>
              Connect…
            </RowAction>
          }
        />
      )
    case "connecting":
      return (
        <ItemRow
          setting="github"
          found={found}
          label={entry.label}
          detail={failed ?? "Enter this code on GitHub to sign in."}
          data-github-state="connecting"
          control={
            <RowAction disabled={busy} onClick={() => act((each) => each.cancel())}>
              Cancel
            </RowAction>
          }
        >
          <PairingCode
            data-github-code={view.userCode}
            code={symbols(view.userCode)}
            expiresAt={deadline.current?.at ?? Date.now()}
            actions={
              <RowLink href={view.verificationUri} data-github-open>
                Open GitHub
              </RowLink>
            }
          />
        </ItemRow>
      )
    case "connected":
      return (
        <>
          <ItemRow
            setting="github"
            found={found}
            label={entry.label}
            detail={
              failed ??
              [`Signed in as @${view.login}.`, view.unrecorded].filter(Boolean).join(" ")
            }
            data-github-state="connected"
            control={
              <span className="linked-actions">
                <RowLink href={view.installUrl} data-github-repositories>
                  Choose repositories
                </RowLink>
                <RowAction
                  disabled={busy}
                  onClick={() => act((each) => each.disconnect())}
                >
                  Disconnect
                </RowAction>
              </span>
            }
          />
          <ItemRow
            data-github-live
            label="Live GitHub events"
            detail={
              view.liveEvents
                ? view.liveEvents.gaps
                  ? "Some offline history expired. Continue with retained events to resume."
                  : `PR, issue and CI updates.${view.liveEvents.enabled ? ` ${view.liveEvents.repositories} ${view.liveEvents.repositories === 1 ? "repository" : "repositories"}.` : ""}`
                : "Not available in this build."
            }
            control={
              <>
                <GithubLiveHelp
                  key="help"
                  refresh={
                    view.liveEvents?.enabled
                      ? () => act((each) => each.liveEvents(true))
                      : undefined
                  }
                  disabled={busy}
                />
                <Toggle
                  key="switch"
                  label="Enable live GitHub events"
                  checked={view.liveEvents?.enabled ?? false}
                  disabled={busy || !view.liveEvents}
                  onChange={(enabled) => act((each) => each.liveEvents(enabled))}
                />
              </>
            }
          >
            {Boolean(view.liveEvents?.gaps) && (
              <RowAction
                disabled={busy}
                onClick={() => act((each) => each.liveEvents(true, true))}
              >
                Continue with retained events
              </RowAction>
            )}
          </ItemRow>
        </>
      )
  }
}

/**
 * The row after a read. A read that fails once the row has an answer keeps
 * that answer, and the code with it, and says why under it until a read
 * answers: the next read (the poll, or focus) tries again. One that answers
 * keeps an action's failure while the state it failed in holds.
 */
function settled(previous: Phase, outcome: GithubOutcome): Phase {
  if (previous.kind !== "ready")
    return outcome.ok
      ? { kind: "ready", view: outcome.view }
      : { kind: "unread", sentence: outcome.sentence }
  if (!outcome.ok) return { ...previous, unread: outcome.sentence }
  return {
    kind: "ready",
    view: outcome.view,
    failed: outcome.view.kind === previous.view.kind ? previous.failed : undefined,
  }
}

/** GitHub's codes are eight symbols with a hyphen between the halves; PairingCode groups them itself. */
function symbols(code: string): string {
  return code.length === 9 && code[4] === "-"
    ? `${code.slice(0, 4)}${code.slice(5)}`
    : code
}
