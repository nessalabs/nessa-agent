/**
 * Settings › Connections › Accounts › Voice: the OpenAI key voice calls are
 * minted with. The gateway keeps it in its vault and never gives it back;
 * the row says only whether one is saved, and offers to save, replace or
 * remove it.
 */
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useState,
  type ReactNode,
} from "react"
import type { VoiceKeyGateway, VoiceKeyOutcome } from "../../voice/adapters/voice-gateway"
import { useVaultUnlock } from "../../../vault"
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

export function VoiceKeyRow() {
  const gateway = useContext(VoiceKeyContext)
  const ensure = useVaultUnlock()
  const entry = setting("voice")
  const found = useContext(FoundSetting) === "voice"
  const keyId = useId()
  const [saved, setSaved] = useState<boolean | undefined>(undefined)
  const [key, setKey] = useState("")
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
    void gateway.read().then((outcome) => current && take(outcome))
    return () => {
      current = false
    }
  }, [gateway])

  const act = async (work: (gateway: VoiceKeyGateway) => Promise<VoiceKeyOutcome>) => {
    if (!gateway || busy) return
    setBusy(true)
    try {
      if (!(await ensure())) {
        setError("Unlock Nessa to change the voice key.")
        return
      }
      const outcome = await work(gateway)
      take(outcome)
      if (outcome.kind === "read") setKey("")
    } finally {
      setBusy(false)
    }
  }

  return (
    <ItemRow
      unavailable={!gateway}
      setting="voice"
      found={found}
      label={entry.label}
      detail={entry.detail}
      control={
        saved ? (
          <span className="settings-boat-status" role="status" data-voice-key="saved">
            Key saved{" "}
            <RowAction
              disabled={busy}
              onClick={() => void act((gateway) => gateway.forget())}
            >
              Remove
            </RowAction>
          </span>
        ) : undefined
      }
    >
      <div className="settings-boat">
        <form
          onSubmit={(event) => {
            event.preventDefault()
            const typed = key.trim()
            if (typed) void act((gateway) => gateway.save(typed))
          }}
        >
          <label htmlFor={keyId}>OpenAI API key</label>
          <div className="settings-boat-fields">
            <input
              id={keyId}
              className="settings-input settings-input-mono"
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
              {busy ? "Saving…" : saved ? "Replace key" : "Save key"}
            </button>
          </div>
        </form>
        {error && <p role="alert">{error}</p>}
      </div>
    </ItemRow>
  )
}
