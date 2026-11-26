/**
 * Settings › Connections › Accounts › Voice: the OpenAI key voice calls are
 * minted with. The gateway keeps it in its vault and never gives it back;
 * the row says only whether one is saved, and offers to add it (in a
 * dialog) or remove it.
 */
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { Button } from "@nessa-ui/react/button"
import { Dialog } from "@nessa-ui/react/dialog"
import type { VoiceKeyGateway, VoiceKeyOutcome } from "../../voice/adapters/voice-gateway"
import { useVaultSettled, useVaultUnlock } from "../../../vault"
import { setting } from "../model/settings-catalogue"
import { FoundSetting, ItemRow, RowAction } from "./settings-controls"

const VoiceKeyContext = createContext<VoiceKeyGateway | undefined>(undefined)

export function VoiceKeyProvider({
  gateway,
  children,
}: {
  gateway: VoiceKeyGateway | undefined
  children: ReactNode
}) {
  return <VoiceKeyContext.Provider value={gateway}>{children}</VoiceKeyContext.Provider>
}

/**
 * The key, asked for in a dialog: its field, Save, and what went wrong. The
 * one place the key is typed, from Settings' row, from a call that found no
 * key (`VoiceKeyButton`), and from Talk before a call that would have been
 * refused for want of one (`forCall`: it says why, and Save starts the call).
 */
export function VoiceKeyDialog({
  onClose,
  forCall = false,
}: {
  onClose: (saved: boolean) => void
  forCall?: boolean
}) {
  const gateway = useContext(VoiceKeyContext)
  const ensure = useVaultUnlock()
  const keyId = useId()
  const [key, setKey] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = async (typed: string) => {
    if (!gateway || busy) return
    setBusy(true)
    try {
      if (!(await ensure())) {
        setError("Unlock Nessa to save the key.")
        return
      }
      const outcome = await gateway.save(typed)
      if (outcome.kind === "read") onClose(outcome.keySaved)
      else setError(outcome.sentence)
    } finally {
      setBusy(false)
    }
  }
  return (
    <Dialog
      open
      size="sm"
      data-voice-key="dialog"
      onOpenChange={(open) => {
        if (!open) onClose(false)
      }}
      title={forCall ? "Add your OpenAI key" : "OpenAI API key"}
      description={
        forCall
          ? "Voice talks through OpenAI, whichever agent runs this chat. The key stays in your vault."
          : undefined
      }
    >
      <form
        className="settings-surface settings-key-form"
        onSubmit={(event) => {
          event.preventDefault()
          const typed = key.trim()
          if (typed) void save(typed)
        }}
      >
        <div className="settings-boat-fields">
          <input
            id={keyId}
            aria-label="OpenAI API key"
            className="settings-input"
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="sk-…"
            value={key}
            onChange={(event) => setKey(event.target.value)}
            disabled={busy || !gateway}
            data-voice-key="input"
          />
          <button
            className="settings-button"
            type="submit"
            disabled={!key.trim() || busy || !gateway}
            data-voice-key="save"
          >
            {busy ? "Saving…" : forCall ? "Save and talk" : "Save"}
          </button>
        </div>
        {error && (
          <p role="alert" className="settings-boat-note">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  )
}

/**
 * A button that asks for the key where its lack was found, then says when
 * it is saved: a call refused for want of a key offers it beside the
 * refusal, instead of sending the person to Settings.
 */
export function VoiceKeyButton({
  children,
  onSaved,
  className,
}: {
  children: ReactNode
  onSaved?: () => void
  className?: string
}) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button
        variant="tinted"
        shape="pill"
        size="26"
        className={className}
        data-voice-key="add"
        onClick={() => setOpen(true)}
      >
        {children}
      </Button>
      {open ? (
        <VoiceKeyDialog
          onClose={(saved) => {
            setOpen(false)
            if (saved) onSaved?.()
          }}
        />
      ) : null}
    </>
  )
}

export function VoiceKeyRow() {
  const gateway = useContext(VoiceKeyContext)
  const ensure = useVaultUnlock()
  const vaultSettled = useVaultSettled()
  const reads = useRef(0)
  const entry = setting("voice")
  const found = useContext(FoundSetting) === "voice"
  const [saved, setSaved] = useState<boolean | undefined>(undefined)
  const [asking, setAsking] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const take = (outcome: VoiceKeyOutcome) => {
    if (outcome.kind === "read") {
      setSaved(outcome.keySaved)
      setError(null)
    } else setError(outcome.sentence)
  }
  useEffect(() => {
    if (!gateway) return
    let current = true
    const revision = ++reads.current
    void gateway.read().then((outcome) => {
      if (current && revision === reads.current) take(outcome)
    })
    return () => {
      current = false
    }
  }, [gateway, vaultSettled])

  const forget = async () => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    try {
      if (!(await ensure())) {
        setError("Unlock Nessa to remove the key.")
        return
      }
      reads.current += 1
      const outcome = await gateway.forget()
      reads.current += 1
      take(outcome)
    } finally {
      setBusy(false)
    }
  }

  // One word of state, and one button: the key is typed in a dialog.
  return (
    <>
      <ItemRow
        unavailable={!gateway}
        setting="voice"
        found={found}
        label={entry.label}
        detail={
          error ??
          (saved === undefined ? undefined : saved ? "Connected" : "Not connected")
        }
        data-voice-key={saved ? "saved" : undefined}
        control={
          saved ? (
            <RowAction disabled={busy} onClick={() => void forget()}>
              Remove
            </RowAction>
          ) : (
            <RowAction disabled={busy} onClick={() => setAsking(true)}>
              Connect…
            </RowAction>
          )
        }
      />
      {asking ? (
        <VoiceKeyDialog
          onClose={(next) => {
            setAsking(false)
            if (next) {
              reads.current += 1
              setSaved(true)
              setError(null)
            }
          }}
        />
      ) : null}
    </>
  )
}
