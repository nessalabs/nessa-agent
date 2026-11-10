/** PeerRead invitations. Choice precedes code creation; every later label comes
 * from the saved owner status. The separate Human session is owned by this page,
 * with scheduled teardown and no automatic mutation replay. See gateway pairing
 * orderings in docs/design/access-grants.md and add-gateway.test.tsx. */
import { Button } from "@nessa-ui/react/button"
import { PageHeader } from "@nessa-ui/react/page-header"
import { useEffect, useRef, useState } from "react"
import type { PairingApi } from "@nessa/client"
import {
  gatewayInvitation,
  type GatewayPairing,
  type PairingScope,
} from "../adapters/gateway-pairing"
import { gatewayEnrollmentCommand } from "../model/gateway-pairing"
import { bytesToHex, deviceKeyFingerprint } from "../../settings/model/device-key"

type PairingOwnerStatus = Awaited<ReturnType<PairingApi["status"]>>
type ActivationStop = Awaited<ReturnType<PairingApi["approve"]>>["activationStopped"]

const SESSION_MS = 10 * 60 * 1_000
const PEER_READ = "peer-gateway-conversation-read"
const phaseText: Record<PairingOwnerStatus["phase"], string> = {
  available: "Waiting for the other gateway",
  claimed: "Waiting for fingerprint approval",
  approved: "Approval saved; activating",
  staging: "Activating",
  active: "Active",
  terminal: "Ended",
}

export function AddGateway({
  gateway,
  onBack,
  port,
}: {
  gateway: GatewayPairing
  onBack: () => void
  port: number
}) {
  const [samePerson, setSamePerson] = useState(false)
  const [recovering, setRecovering] = useState(false)
  const [token, setToken] = useState("")
  const [signedIn, setSignedIn] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState("")
  const [needsRead, setNeedsRead] = useState(true)
  const [pending, setPending] = useState<readonly PairingOwnerStatus[]>([])
  const [status, setStatus] = useState<PairingOwnerStatus>()
  const [code, setCode] = useState<string>()
  const [fingerprint, setFingerprint] = useState<string>()
  const [activationStopped, setActivationStopped] = useState<ActivationStop>()
  const page = useRef<HTMLDivElement | null>(null)
  const createButton = useRef<HTMLButtonElement | null>(null)
  const scope = useRef<PairingScope | undefined>(undefined)
  const controller = useRef<AbortController | undefined>(undefined)
  const operation = useRef<object | undefined>(undefined)
  const generation = useRef(0)
  const deadline = useRef<(() => void) | undefined>(undefined)

  function retire() {
    generation.current += 1
    operation.current = undefined
    controller.current?.abort()
    controller.current = undefined
    scope.current?.close()
    scope.current = undefined
    deadline.current?.()
    deadline.current = undefined
  }
  useEffect(() => {
    page.current?.focus()
  }, [])
  useEffect(() => {
    if (signedIn && !status && !needsRead && !busy) createButton.current?.focus()
    if (!signedIn && status) page.current?.focus()
  }, [signedIn, status, needsRead, busy])
  useEffect(() => {
    // Reload and back/forward-cache entry need teardown even when React does
    // not unmount. pagehide also retires an unresolved connection attempt.
    const hide = () => {
      retire()
      setToken("")
      setSignedIn(false)
      setBusy(false)
      setCode(undefined)
      setStatus(undefined)
      setPending([])
      setMessage("Pairing sign-in ended. Continue again to read pending invitations.")
    }
    window.addEventListener("pagehide", hide)
    return () => {
      window.removeEventListener("pagehide", hide)
      retire()
    }
  }, [])

  // No polling response can race an approve/cancel and replace its newer answer.
  async function run(action: () => Promise<void>) {
    if (operation.current) return
    const ticket = {}
    operation.current = ticket
    setBusy(true)
    const current = generation.current
    try {
      await action()
    } catch {
      if (current === generation.current) {
        setNeedsRead(true)
        setMessage(
          "Not confirmed. Go back and reopen Gateway to resume saved invitations.",
        )
      }
    } finally {
      if (operation.current === ticket) {
        operation.current = undefined
        if (current === generation.current) setBusy(false)
      }
    }
  }

  async function connect(owner = samePerson || recovering) {
    const credential = owner ? token.trim() : undefined
    setToken("")
    const current = generation.current
    const abort = new AbortController()
    controller.current = abort
    const opened = await gateway.open(credential, abort.signal)
    if (current !== generation.current) return opened.close()
    scope.current = opened
    setSignedIn(true)
    setMessage("")
    deadline.current = gateway.after(SESSION_MS, () => {
      retire()
      setSignedIn(false)
      setBusy(false)
      setCode(undefined)
      setStatus(undefined)
      setPending([])
      setMessage("Pairing sign-in ended. Continue again to read pending invitations.")
    })
    await readPending(opened, current)
  }

  function currentScope() {
    if (!scope.current) throw new DOMException("Pairing closed", "AbortError")
    return scope.current
  }

  async function readPending(opened = scope.current, current = generation.current) {
    if (!opened) return
    const reply = await opened.pairing.pending()
    if (current !== generation.current) return
    setPending(reply.items.filter((each) => each.class === PEER_READ))
    setNeedsRead(false)
    setMessage("")
  }

  function finishReceipt() {
    retire()
    setSignedIn(false)
    setBusy(false)
    setCode(undefined)
  }

  function apply(next: PairingOwnerStatus, { resuming = false } = {}) {
    setStatus(next)
    setMessage("")
    if (next.phase === "active") setCode(undefined)
    if (next.phase === "terminal") {
      setCode(undefined)
      setActivationStopped(undefined)
      // Only a fresh, explicitly resumed unfinished receipt keeps a scope
      // for Finish removal. Polling an ended invitation retires its old one.
      if (!resuming || !next.cleanupPending) finishReceipt()
    }
  }

  const invitation = status && bytesToHex(status.invitationId)
  useEffect(() => {
    setFingerprint(undefined)
    if (!status?.claimedDeviceKey) return
    let live = true
    const current = generation.current
    void deviceKeyFingerprint(Uint8Array.from(status.claimedDeviceKey))
      .then((value) => {
        if (live && current === generation.current) setFingerprint(value)
      })
      .catch(() => {
        if (live && current === generation.current)
          setMessage(
            "Fingerprint unavailable. Read this invitation again before approving.",
          )
      })
    return () => {
      live = false
    }
  }, [status?.claimedDeviceKey])

  useEffect(() => {
    if (!signedIn || !status || status.phase === "terminal") return
    const id = status.invitationId
    const opened = scope.current
    const readGeneration = generation.current
    let cancel: (() => void) | undefined
    let live = true
    const tick = () => {
      if (!live || !opened || readGeneration !== generation.current) return
      void run(async () => {
        const current = generation.current
        const next = await opened.pairing.status(id)
        if (current === generation.current) apply(next)
      }).finally(() => {
        if (live && readGeneration === generation.current)
          cancel = gateway.after(2_000, tick)
      })
    }
    cancel = gateway.after(2_000, tick)
    return () => {
      live = false
      cancel?.()
    }
    // The id is the durable selection; polling cannot choose a different record.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signedIn, invitation])

  return (
    <div
      ref={page}
      tabIndex={-1}
      role="region"
      aria-label="Pair a gateway"
      className="devices-page devices-add-page devices-gateway-page"
      data-devices-page="add-gateway"
    >
      <button className="devices-back" type="button" onClick={onBack}>
        Back
      </button>
      <div className="devices-heading">
        <PageHeader title="Pair a gateway" />
        <p className="devices-muted">
          Connect another Nessa gateway to read conversations you share with it.
        </p>
      </div>
      <p className="devices-muted">
        First enable Native linking in Settings → Connections → Linked devices so the
        other gateway can reach this one.
      </p>
      {!signedIn && !status && (
        <form
          className="devices-card"
          onSubmit={(event) => {
            event.preventDefault()
            void run(() => connect())
          }}
        >
          {!recovering && (
            <label>
              <input
                type="checkbox"
                checked={samePerson}
                disabled={busy}
                onChange={(event) => setSamePerson(event.target.checked)}
              />
              Both gateways belong to me
            </label>
          )}
          {!recovering && (
            <p className="devices-muted">
              This records the same person. It gives no access to conversations.
            </p>
          )}
          {recovering && (
            <p>Read existing invitations. Their person choices are already saved.</p>
          )}
          {(samePerson || recovering) && (
            <label className="devices-gateway-field">
              Owner access token
              <input
                type="password"
                className="devices-gateway-input"
                value={token}
                autoComplete="off"
                spellCheck={false}
                disabled={busy}
                onChange={(event) => setToken(event.target.value)}
              />
              <span className="devices-muted">
                Explicit owner sign-in for pairing only. Your ordinary panel sign-in stays
                in place. The token is not saved.
              </span>
            </label>
          )}
          <Button
            className="devices-primary"
            variant="inverse"
            size="30"
            shape="pill"
            type="submit"
            disabled={busy || ((samePerson || recovering) && !token.trim())}
          >
            Continue
          </Button>
          {!recovering ? (
            <>
              <Button
                variant="tinted"
                size="30"
                shape="pill"
                type="button"
                disabled={busy}
                onClick={() => {
                  setRecovering(true)
                  setToken("")
                }}
              >
                Resume owner invitations
              </Button>
              <Button
                variant="tinted"
                size="30"
                shape="pill"
                type="button"
                disabled={busy}
                onClick={() => {
                  setRecovering(true)
                  void run(() => connect(false))
                }}
              >
                Resume panel invitations
              </Button>
            </>
          ) : (
            <Button
              variant="tinted"
              size="30"
              shape="pill"
              type="button"
              disabled={busy}
              onClick={() => {
                setRecovering(false)
                setToken("")
              }}
            >
              New invitation
            </Button>
          )}
        </form>
      )}
      {signedIn && !status && (
        <div>
          {!recovering && (
            <>
              <p>New invitation: {samePerson ? "same person" : "no person link"}.</p>
              <Button
                ref={createButton}
                className="devices-primary"
                variant="inverse"
                size="30"
                shape="pill"
                disabled={busy || needsRead}
                onClick={() =>
                  void run(async () => {
                    const current = generation.current
                    const made = await currentScope().pairing.create(
                      gatewayInvitation(samePerson),
                    )
                    if (current !== generation.current) return
                    setCode(made.code)
                    apply(made.status)
                  })
                }
              >
                Create invitation
              </Button>
            </>
          )}
          <Button
            variant="tinted"
            size="30"
            shape="pill"
            disabled={busy}
            onClick={() => void run(() => readPending())}
          >
            Read pending invitations
          </Button>
          {pending.map((each) => (
            <Button
              key={bytesToHex(each.invitationId)}
              variant="tinted"
              size="30"
              shape="pill"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  const current = generation.current
                  const next = await currentScope().pairing.status(each.invitationId)
                  if (current !== generation.current) return
                  setCode(undefined)
                  setActivationStopped(undefined)
                  apply(next, { resuming: true })
                  page.current?.focus()
                })
              }
            >
              Resume {bytesToHex(each.invitationId)} —{" "}
              {each.samePerson === undefined
                ? "choice unknown"
                : each.samePerson
                  ? "same person"
                  : "no person link"}{" "}
              — {phaseText[each.phase]}
            </Button>
          ))}
        </div>
      )}
      {status && (
        <div className="devices-card" data-gateway-invitation={invitation}>
          <p data-gateway-choice>
            Saved choice:{" "}
            {status.samePerson === undefined
              ? "unknown"
              : status.samePerson
                ? "same person"
                : "no person link"}
          </p>
          <p data-gateway-phase>Pairing: {phaseText[status.phase]}</p>
          {code ? (
            <div>
              <p>
                In a POSIX shell on the other gateway, run this with this gateway’s native
                IP address and port in place of GATEWAY_IP:{port}:
              </p>
              <pre className="devices-gateway-command">
                {gatewayEnrollmentCommand(port, code)}
              </pre>
              <p>
                One-use code: <code data-gateway-code>{code}</code>
              </p>
            </div>
          ) : status.phase === "terminal" ? (
            <p>This invitation has ended. Its code can no longer be used.</p>
          ) : status.phase !== "active" ? (
            <p>
              The code is shown only when created. Cancel this invitation to make a
              different choice or get a new code.
            </p>
          ) : null}
          {fingerprint && (
            <p>
              Compare this fingerprint with the other gateway before approving:{" "}
              <code data-gateway-fingerprint>{fingerprint}</code>
            </p>
          )}
          {activationStopped === "retryable" && (
            <p role="status">
              Activation needs another attempt. Person attribution is not confirmed.
            </p>
          )}
          {activationStopped === "permanent" && (
            <p role="status">
              Activation cannot complete. Cancel this invitation and pair again. Person
              attribution is not confirmed.
            </p>
          )}
          {status.phase === "active" && (
            <p>
              Pairing is active. Reads still need a separate share. The saved choice does
              not confirm a committed person link.
            </p>
          )}
          {status.cleanupPending && (
            <p>Cleanup is still pending. Resume this invitation to finish removal.</p>
          )}
          {signedIn && (
            <div>
              {status.claimedDeviceKey &&
                status.phase !== "terminal" &&
                activationStopped !== "permanent" && (
                  <Button
                    className="devices-primary"
                    variant="inverse"
                    size="30"
                    shape="pill"
                    disabled={busy || !fingerprint}
                    onClick={() =>
                      void run(async () => {
                        const current = generation.current
                        const key = status.claimedDeviceKey
                        if (!key) return
                        const reply = await currentScope().pairing.approve(
                          status.invitationId,
                          key,
                        )
                        if (current !== generation.current) return
                        setActivationStopped(reply.activationStopped)
                        apply(reply.status)
                        if (reply.status.phase === "active" && !reply.activationStopped) {
                          finishReceipt()
                        }
                      })
                    }
                  >
                    {status.phase === "claimed"
                      ? "Approve matching fingerprint"
                      : "Retry activation"}
                  </Button>
                )}
              <Button
                variant="tinted"
                size="30"
                shape="pill"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    const current = generation.current
                    const next = await currentScope().pairing.cancel(status.invitationId)
                    if (current !== generation.current) return
                    apply(next)
                  })
                }
              >
                {status.phase === "terminal" ? "Finish removal" : "Cancel invitation"}
              </Button>
            </div>
          )}
        </div>
      )}
      {message && <p role="alert">{message}</p>}
      {signedIn && (
        <Button
          variant="tinted"
          size="30"
          shape="pill"
          onClick={() => {
            retire()
            setSignedIn(false)
            setBusy(false)
            setCode(undefined)
            setStatus(undefined)
            setPending([])
            setRecovering(false)
            setMessage(
              "Signed out of pairing. Unfinished invitations remain recoverable.",
            )
          }}
        >
          End pairing sign-in
        </Button>
      )}
    </div>
  )
}
