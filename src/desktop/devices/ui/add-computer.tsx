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
 * no other computer can reach the hub ──▶ "Let other computers reach it"
 *   ──▶ pairing.setLinking(true) ──▶ the gateway starts again listening on
 *   every interface ──▶ read until it says so ──▶ a new code
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

/**
 * The loopback address Settings once saved for the native listener. Turning
 * linking on moves it to every interface; any other loopback address was
 * chosen by hand and is left for the person to change.
 */
const SUPERSEDED_LISTEN = "127.0.0.1:47650"

/**
 * Whether turning native linking on lets other computers reach the hub:
 * linking is off (`null`), or the listener is on the address Settings once
 * saved for this machine alone.
 */
export function reachable(listening: string | null | undefined): boolean {
  return listening === null || listening === SUPERSEDED_LISTEN
}

/** How long the page waits for a restarting hub to listen for others. */
const REACH_WAIT_MS = 30_000
const REACH_READ_MS = 1_000

/** How the computer reaches the hub. */
export type Route = "tailscale" | "network"

/**
 * The port in `listening`, where the hub says its native listener is bound
 * (`host:port`, `[v6]:port`); [`NATIVE_PORT`] when it says none.
 */
export function listeningPort(listening: string | undefined): number {
  const port = Number(/:(\d{1,5})$/.exec(listening ?? "")?.[1])
  return Number.isInteger(port) && port > 0 && port < 65_536 ? port : NATIVE_PORT
}

/** Whether `listening` is bound to this machine alone: no other can dial it. */
export function onlyThisMachine(listening: string | undefined): boolean {
  const host = listening?.replace(/:\d{1,5}$/, "").replace(/^\[(.*)\]$/, "$1")
  return (
    host !== undefined && (/^127\./.test(host) || host === "::1" || host === "localhost")
  )
}

/** Where a computer dials the hub `hub` names, by route, at `port`. */
export function hubAddress(hub: string, route: Route, port = NATIVE_PORT): string {
  const host =
    hub
      .trim()
      .toLowerCase()
      .replace(/\.local$/, "")
      // As Tailscale and Bonjour name a machine: "Ada’s Mac" is adas-mac.
      .replace(/['’]/g, "")
      .replace(/[^a-z0-9.-]+/g, "-") || "your-mac"
  return route === "tailscale" ? `${host}:${port}` : `${host}.local:${port}`
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
  listening,
  onReached,
  onDone,
}: {
  gateway: DevicesGateway
  hubName: string | undefined
  /**
   * Where the hub says its native listener is bound (`devices.list`);
   * `null` while linking is off, `undefined` before the list is read.
   */
  listening: string | null | undefined
  /** The hub now listens for other computers: read the list again. */
  onReached: () => void
  onDone: () => void
}) {
  const [route, setRoute] = useState<Route>("tailscale")
  const [reach, setReach] = useState<"idle" | "reaching" | "failed">("idle")
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const [step, setStep] = useState<Step>({ kind: "making" })
  const [attempt, setAttempt] = useState(0)
  // The invitation still open, cancelled if the page closes before approval.
  const open = useRef<string | null>(null)
  // The hub makes one code at a time and refuses another while it does
  // (`pairing_busy`): each request waits for the last, and its cancel, to end.
  const settled = useRef<Promise<unknown>>(Promise.resolve())
  // The invitation the page shows now; `null` while making one, or gone.
  const shown = useRef<string | null>(null)

  useEffect(() => {
    let live = true
    setStep({ kind: "making" })
    // A new code replaces the one shown: that one can no longer be used.
    const replaced = open.current
    open.current = null
    shown.current = null
    const made = settled.current
      .then(() => (replaced ? gateway.cancel(replaced) : undefined))
      .then(() => gateway.pair())
    settled.current = made.then((each) =>
      !live && each.ok ? gateway.cancel(each.value.pairing.invitationId) : undefined,
    )
    void made.then((each) => {
      if (!live) return
      if (!each.ok) return setStep({ kind: "failed" })
      open.current = each.value.pairing.invitationId
      shown.current = each.value.pairing.invitationId
      setStep({ kind: "waiting", code: each.value, pairing: each.value.pairing })
    })
    return () => {
      live = false
    }
  }, [gateway, attempt])

  useEffect(
    () => () => {
      shown.current = null
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
    const id = pairing.invitationId
    // Being approved, it is no longer the page's to cancel on leaving.
    open.current = null
    setStep({ kind: "approving", code, pairing })
    void gateway.approve(id, pairing.deviceKey).then((approved) => {
      const added = approved.ok && approved.value.phase !== "terminal"
      if (shown.current !== id) {
        // The page left, or showed a new code, while it was approved: one
        // that was not is still open, and nobody else will cancel it.
        if (!added) void gateway.cancel(id)
        return
      }
      if (added) return setStep({ kind: "added", code })
      open.current = id
      setStep({ kind: "waiting", code, pairing: approved.ok ? approved.value : pairing })
    })
  }

  const letReach = async () => {
    setReach("reaching")
    const turned = await gateway.reach()
    // A restarting hub answers again once it listens where it now says.
    const deadline = Date.now() + REACH_WAIT_MS
    let listens = turned.ok && turned.value === "unchanged"
    while (turned.ok && !listens && mounted.current && Date.now() < deadline) {
      await new Promise((resolve) => window.setTimeout(resolve, REACH_READ_MS))
      const read = await gateway.list()
      // Listening, and not on the address the change moves: the hub that
      // answers has started again with it.
      listens =
        read.ok &&
        read.value.listening !== undefined &&
        read.value.listening !== SUPERSEDED_LISTEN
    }
    if (!mounted.current) return
    if (!listens) return setReach("failed")
    setReach("idle")
    onReached()
    setAttempt((value) => value + 1)
  }

  // Turning linking on is offered where the person meets the problem,
  // never as a trip to Settings.
  const reachBlock = (
    <div className="devices-actions" data-devices-reach>
      <p className="devices-muted" role="status">
        {reach === "failed"
          ? `${hubName ?? "The hub"} didn't start listening for other computers. Try again.`
          : step.kind === "failed"
            ? `The hub couldn't make a code: no other computer can reach ${hubName ?? "it"} yet.`
            : `No other computer can reach ${hubName ?? "the hub"} yet.`}
      </p>
      <Button
        variant="tinted"
        size="30"
        shape="pill"
        disabled={reach === "reaching"}
        onClick={() => void letReach()}
        data-devices-action="reach"
      >
        {reach === "reaching" ? "Starting again" : "Let other computers reach it"}
      </Button>
    </div>
  )

  const address = hubAddress(hubName ?? "", route, listeningPort(listening ?? undefined))
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
            {!reachable(listening) && onlyThisMachine(listening ?? undefined) ? (
              <p className="devices-muted" role="status" data-devices-loopback>
                {hubName ?? "The hub"} listens only to itself now ({listening}), so no
                other computer can reach it yet. Set native.listenAddress in its
                config.json to 0.0.0.0:{listeningPort(listening ?? undefined)}, then turn
                native linking off and on in Settings › Linked devices.
              </p>
            ) : null}
          </section>

          <section className="devices-card">
            <h2>2. On the computer, run</h2>
            {step.kind === "making" ? (
              <LoadingText>Making a code</LoadingText>
            ) : step.kind === "failed" && reachable(listening) ? null : step.kind ===
              "failed" ? (
              <div className="devices-actions">
                <p className="devices-muted" role="status">
                  The hub couldn&apos;t make a code.
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
                      disabled={step.kind === "approving"}
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
            {/* Its own slot, so the button keeps focus as the step above changes. */}
            {reachable(listening) ? reachBlock : null}
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
