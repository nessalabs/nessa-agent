/**
 * Add a computer: a pairing code for it, the commands that enroll it and
 * then serve the hub, and the approval of the key it claimed. Who may pair,
 * and when a code ends, is the gateway's to say; this shows its answers.
 *
 * ```text
 * open ──▶ pairing.create(environment) ──▶ code shown with the commands
 * the computer runs `nessa env enroll` ──▶ status: claimed + key fingerprint
 * Approve ──▶ pairing.approve(key) ──▶ it runs `nessa env dial` and serves
 * leave before approving ──▶ pairing.cancel
 * ```
 */
import { Button } from "@nessa-ui/react/button"
import { KeyFingerprint } from "@nessa-ui/react/key-fingerprint"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { PairingCode } from "@nessa-ui/react/pairing-code"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { useEffect, useRef, useState } from "react"
import { useClipboardWrite } from "../../../conversation"
import type {
  DevicesGateway,
  WorkerCode,
  WorkerPairing,
} from "../adapters/devices-gateway"

/** The port the hub's native listener takes when none is named. */
export const NATIVE_PORT = 47650

/** How the computer reaches the hub. */
export type Route = "tailscale" | "network"

/** Where a computer dials the hub `hub` names, by route. */
export function hubAddress(hub: string, route: Route): string {
  const host =
    hub
      .trim()
      .toLowerCase()
      .replace(/\.local$/, "")
      // As Tailscale and Bonjour name a machine: "Ada’s Mac" is adas-mac.
      .replace(/['’]/g, "")
      .replace(/[^a-z0-9.-]+/g, "-") || "your-mac"
  return route === "tailscale" ? `${host}:${NATIVE_PORT}` : `${host}.local:${NATIVE_PORT}`
}

/** The two commands a computer runs, in order. */
export function commands(address: string, code: string): readonly [string, string] {
  return [`nessa env enroll ${address} ${code}`, `nessa env dial ${address}`]
}

/** How often a shown code's pairing is read again. */
const STATUS_MS = 2_000

type Step =
  | { readonly kind: "making" }
  | { readonly kind: "failed" }
  | {
      readonly kind: "waiting"
      readonly code: WorkerCode
      readonly pairing: WorkerPairing
    }
  | {
      readonly kind: "approving"
      readonly code: WorkerCode
      readonly pairing: WorkerPairing
    }
  | { readonly kind: "added"; readonly code: WorkerCode }

const back = (
  <svg
    viewBox="0 0 16 16"
    width="14"
    height="14"
    aria-hidden="true"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <path d="M10 3 5 8l5 5" />
  </svg>
)

export function AddComputer({
  gateway,
  hubName,
  onDone,
}: {
  gateway: DevicesGateway
  hubName: string | undefined
  onDone: () => void
}) {
  const [route, setRoute] = useState<Route>("tailscale")
  const [step, setStep] = useState<Step>({ kind: "making" })
  const [attempt, setAttempt] = useState(0)
  // The invitation still open, cancelled if the page closes before approval.
  const open = useRef<string | null>(null)
  // The hub makes one code at a time and refuses another while it does
  // (`pairing_busy`): each request waits for the last, and its cancel, to end.
  const settled = useRef<Promise<unknown>>(Promise.resolve())

  useEffect(() => {
    let live = true
    setStep({ kind: "making" })
    // A new code replaces the one shown: that one can no longer be used.
    const shown = open.current
    open.current = null
    const made = settled.current
      .then(() => (shown ? gateway.cancel(shown) : undefined))
      .then(() => gateway.pair())
    settled.current = made.then((each) =>
      !live && each.ok ? gateway.cancel(each.value.pairing.invitationId) : undefined,
    )
    void made.then((each) => {
      if (!live) return
      if (!each.ok) return setStep({ kind: "failed" })
      open.current = each.value.pairing.invitationId
      setStep({ kind: "waiting", code: each.value, pairing: each.value.pairing })
    })
    return () => {
      live = false
    }
  }, [gateway, attempt])

  useEffect(
    () => () => {
      if (open.current) void gateway.cancel(open.current)
    },
    [gateway],
  )

  const invitation = step.kind === "waiting" ? step.pairing.invitationId : null
  useEffect(() => {
    if (!invitation) return
    const timer = window.setInterval(() => {
      void gateway.status(invitation).then((read) => {
        if (!read.ok) return
        setStep((last) =>
          last.kind === "waiting" && last.pairing.invitationId === invitation
            ? { ...last, pairing: read.value }
            : last,
        )
      })
    }, STATUS_MS)
    return () => window.clearInterval(timer)
  }, [gateway, invitation])

  const approve = (code: WorkerCode, pairing: WorkerPairing) => {
    if (!pairing.deviceKey) return
    setStep({ kind: "approving", code, pairing })
    void gateway.approve(pairing.invitationId, pairing.deviceKey).then((approved) => {
      if (approved.ok && approved.value.phase !== "terminal") {
        open.current = null
        setStep({ kind: "added", code })
      } else
        setStep({
          kind: "waiting",
          code,
          pairing: approved.ok ? approved.value : pairing,
        })
    })
  }

  const address = hubAddress(hubName ?? "", route)
  return (
    <div className="devices-page devices-add-page" data-devices-page="add">
      <button
        type="button"
        className="devices-back"
        onClick={onDone}
        data-devices-action="back"
      >
        {back}
        Devices
      </button>
      <header className="devices-heading">
        <h1>Add a computer</h1>
        <p className="devices-muted">
          It runs agents the hub sends it, with its own files and tools. Nessa must be
          installed on it.
        </p>
      </header>

      {step.kind === "added" ? (
        <section className="devices-card" data-devices-added>
          <h2>Approved</h2>
          <p className="devices-muted">
            If it is still waiting, start it serving the hub:
          </p>
          <Command text={commands(address, step.code.code)[1]} />
          <div className="devices-actions">
            <Button
              className="devices-primary"
              variant="inverse"
              size="30"
              shape="pill"
              onClick={onDone}
              data-devices-action="done"
            >
              Done
            </Button>
          </div>
        </section>
      ) : (
        <>
          <section className="devices-card">
            <h2>1. Choose how it reaches {hubName ?? "the hub"}</h2>
            <SegmentedControl
              variant="glass"
              value={route}
              aria-label="How it reaches the hub"
              onValueChange={(next) =>
                setRoute(next === "network" ? "network" : "tailscale")
              }
            >
              <SegmentedControlOption value="tailscale">Tailscale</SegmentedControlOption>
              <SegmentedControlOption value="network">
                Same network
              </SegmentedControlOption>
            </SegmentedControl>
            <p className="devices-muted" data-devices-route-hint>
              {route === "tailscale"
                ? "Recommended. Both machines signed in to the same tailnet reach each other from anywhere, with nothing open to the internet."
                : "Only while both are on the same Wi-Fi or office network."}
            </p>
          </section>

          <section className="devices-card">
            <h2>2. On the computer, run</h2>
            {step.kind === "making" ? (
              <LoadingText>Making a code</LoadingText>
            ) : step.kind === "failed" ? (
              <div className="devices-actions">
                <p className="devices-muted" role="status">
                  The hub couldn&apos;t make a code. Native linking may be off in Settings
                  › Linked devices.
                </p>
                <Button
                  variant="tinted"
                  size="30"
                  shape="pill"
                  onClick={() => setAttempt((value) => value + 1)}
                >
                  Try again
                </Button>
              </div>
            ) : (
              <>
                <PairingCode
                  code={step.code.code.replace("-", "")}
                  expiresAt={step.code.pairing.expiresAtMs}
                  state={
                    step.pairing.phase === "terminal"
                      ? "cancelled"
                      : step.pairing.phase === "available"
                        ? "open"
                        : "used"
                  }
                  onExpire={() => undefined}
                  actions={
                    <Button
                      variant="plain"
                      size="30"
                      shape="pill"
                      onClick={() => setAttempt((value) => value + 1)}
                    >
                      New code
                    </Button>
                  }
                />
                {commands(address, step.code.code).map((line) => (
                  <Command key={line} text={line} />
                ))}
              </>
            )}
          </section>

          {step.kind === "waiting" || step.kind === "approving" ? (
            <section className="devices-card" data-devices-approve>
              <h2>3. Approve it</h2>
              {step.pairing.fingerprint && step.pairing.phase === "claimed" ? (
                <>
                  <p className="devices-muted">
                    A computer used the code. Approve it if it is the one you just ran the
                    command on.
                  </p>
                  <KeyFingerprint value={step.pairing.fingerprint} />
                  <div className="devices-actions">
                    <Button
                      className="devices-primary"
                      variant="inverse"
                      size="30"
                      shape="pill"
                      disabled={step.kind === "approving"}
                      onClick={() => approve(step.code, step.pairing)}
                      data-devices-action="approve"
                    >
                      Approve
                    </Button>
                  </div>
                </>
              ) : (
                <LoadingText>Waiting for the computer</LoadingText>
              )}
            </section>
          ) : null}
        </>
      )}
    </div>
  )
}

function Command({ text }: { text: string }) {
  const write = useClipboardWrite()
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 1500)
    return () => window.clearTimeout(timer)
  }, [copied])
  return (
    <div className="devices-command" data-devices-command>
      <code>{text}</code>
      {write ? (
        <Button
          variant="plain"
          size="30"
          shape="pill"
          onClick={() =>
            void write(text).then(
              () => setCopied(true),
              () => undefined,
            )
          }
        >
          {copied ? "Copied" : "Copy"}
        </Button>
      ) : null}
    </div>
  )
}
