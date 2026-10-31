import { useId, useState, type FormEvent } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { Button } from "@nessa-ui/react/button"
import { Input } from "@nessa-ui/react/input"
import contract from "../../../protocol/defaults/vault-contract.json"
import type { VaultChangeView } from "../application/change-controller"
import { vaultFailureCopy } from "./failure-copy"
import "./vault.css"
/** New input lives only in this mounted form and is cleared before dispatch. */
export function VaultChangeDialog({
  view,
  container,
  onSubmit,
  onClose,
  onUnlock,
}: {
  view: Exclude<VaultChangeView, { kind: "closed" }>
  container: HTMLElement | null
  onSubmit: (passphrase: string) => Promise<void>
  onClose: () => void
  onUnlock: () => void
}) {
  const id = useId()
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [mismatch, setMismatch] = useState(false)
  const busy = view.kind === "form" && view.busy
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (view.kind !== "form" || busy) return
    if (passphrase !== confirmation) {
      setMismatch(true)
      return
    }
    const input = passphrase
    setPassphrase("")
    setConfirmation("")
    setMismatch(false)
    void onSubmit(input)
  }
  return (
    <Dialog
      container={container}
      open
      title="Change vault passphrase"
      description="Changing your passphrase locks the vault. Unlock again afterward."
      onOpenChange={(open) => {
        if (!open && !busy) {
          setPassphrase("")
          setConfirmation("")
          onClose()
        }
      }}
    >
      <div className="vault-dialog" data-vault-change={view.kind}>
        {view.kind === "finished" ? (
          <>
            <p role={view.failure ? "alert" : "status"}>
              {view.failure
                ? vaultFailureCopy[view.failure]
                : "Your passphrase changed. Your vault is locked; unlock with the new passphrase."}
            </p>
            <div className="vault-actions">
              <Button variant="ghost" onClick={onClose}>
                Close
              </Button>
              <Button onClick={onUnlock}>Unlock vault</Button>
            </div>
          </>
        ) : (
          <form onSubmit={submit}>
            <label htmlFor={`${id}-new`}>New passphrase</label>
            <Input
              id={`${id}-new`}
              type="password"
              autoComplete="new-password"
              autoFocus
              value={passphrase}
              disabled={busy}
              aria-describedby={`${id}-hint`}
              onChange={(event) => {
                setPassphrase(event.target.value)
                setMismatch(false)
              }}
            />
            <label htmlFor={`${id}-confirm`}>Confirm new passphrase</label>
            <Input
              id={`${id}-confirm`}
              type="password"
              autoComplete="new-password"
              value={confirmation}
              disabled={busy}
              onChange={(event) => {
                setConfirmation(event.target.value)
                setMismatch(false)
              }}
            />
            <p id={`${id}-hint`} className="vault-hint">
              Use {contract.minimumSetupCharacters}+ characters. Choose unique words;
              avoid common passwords and repeated text. This does not revoke copied
              credentials or protect older backups.
            </p>
            {mismatch ? <p role="alert">The passphrases don't match.</p> : null}
            {view.failure ? <p role="alert">{vaultFailureCopy[view.failure]}</p> : null}
            <div className="vault-actions">
              <Button type="button" variant="ghost" onClick={onClose} disabled={busy}>
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={
                  busy ||
                  Array.from(passphrase).length < contract.minimumSetupCharacters ||
                  new TextEncoder().encode(passphrase).length >
                    contract.maximumPassphraseBytes
                }
              >
                {busy ? "Changing…" : "Change passphrase"}
              </Button>
            </div>
          </form>
        )}
      </div>
    </Dialog>
  )
}
