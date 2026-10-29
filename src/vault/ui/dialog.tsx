import { useId, useState, type FormEvent } from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { Button } from "@nessa-ui/react/button"
import { Input } from "@nessa-ui/react/input"
import contract from "../../../protocol/defaults/vault-contract.json"
import type { VaultDialog as View } from "../application/controller"
import type { VaultFailure } from "../application/ports"
import "./vault.css"
const copy: Record<VaultFailure, string> = {
  vault_unavailable: "Secure storage is unavailable. Try again.",
  vault_locked: "Your vault is locked.",
  vault_passphrase_policy: `Use at least ${contract.minimumCharacters} characters, including a non-space character.`,
  vault_wrong_passphrase:
    "That passphrase didn't unlock the vault. Check it and try again.",
  vault_busy: "Secure storage is busy. Try again shortly.",
  vault_cooldown: `Too many attempts. Wait ${contract.cooldownSeconds} seconds, then try again.`,
  vault_session_closed: "The app connection ended. Try again.",
  vault_session_owned:
    "Another app session has the vault unlocked. Close it and try again.",
  vault_already_configured: "A vault already exists. Unlock it to continue.",
  vault_capacity: "Secure storage is full.",
  vault_audit_unavailable: "Nessa couldn't record the change. Try again.",
  vault_migration_unavailable:
    "Nessa couldn't import existing protected data. Unlock the old OS credential store once, then try again. Your existing data is preserved.",
}
/** Presentation owns only transient input; submitting or dismissing clears it. */
export function VaultDialog({
  view,
  container,
  onSubmit,
  onCancel,
  onRetry,
}: {
  view: Exclude<View, { kind: "closed" }>
  container: HTMLElement | null
  onSubmit: (passphrase: string) => Promise<void>
  onCancel: () => void
  onRetry: () => void
}) {
  const [passphrase, setPassphrase] = useState("")
  const [confirmation, setConfirmation] = useState("")
  const [help, setHelp] = useState(false)
  const [mismatch, setMismatch] = useState(false)
  const id = useId()
  const setup = view.kind === "setup"
  const ready = view.kind === "setup" || view.kind === "unlock"
  const busy = ready && view.busy
  const title = setup ? "Create your Nessa vault" : "Unlock Nessa"
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!ready || busy) return
    if (setup && passphrase !== confirmation) {
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
      title={title}
      description={
        setup
          ? "Protect saved GitHub credentials and local events."
          : "Enter your passphrase to use protected data."
      }
      open
      onOpenChange={(open) => {
        if (open) return
        setPassphrase("")
        setConfirmation("")
        onCancel()
      }}
    >
      <div className="vault-dialog" data-vault-dialog={view.kind}>
        {view.kind === "checking" ? (
          <p role="status">Opening secure storage…</p>
        ) : view.kind === "unavailable" ? (
          <>
            <p role="alert">{copy[view.failure]}</p>
            <Button onClick={onRetry}>Try again</Button>
          </>
        ) : (
          <form onSubmit={submit}>
            <label htmlFor={`${id}-passphrase`}>Passphrase</label>
            <Input
              id={`${id}-passphrase`}
              type="password"
              autoComplete={setup ? "new-password" : "current-password"}
              autoFocus
              value={passphrase}
              disabled={busy}
              onChange={(event) => {
                setPassphrase(event.target.value)
                setMismatch(false)
              }}
              aria-describedby={`${id}-hint`}
            />
            {setup ? (
              <>
                <label htmlFor={`${id}-confirmation`}>Confirm passphrase</label>
                <Input
                  id={`${id}-confirmation`}
                  type="password"
                  autoComplete="new-password"
                  value={confirmation}
                  disabled={busy}
                  onChange={(event) => {
                    setConfirmation(event.target.value)
                    setMismatch(false)
                  }}
                />
              </>
            ) : null}
            <p id={`${id}-hint`} className="vault-hint">
              {setup
                ? `Use ${contract.minimumCharacters}+ characters or a few words.`
                : "Unlock once until Nessa quits."}
            </p>
            {mismatch ? <p role="alert">The passphrases don't match.</p> : null}
            {view.failure ? <p role="alert">{copy[view.failure]}</p> : null}
            <div className="vault-actions">
              <button
                type="button"
                className="vault-help"
                aria-label="About your vault"
                aria-expanded={help}
                aria-controls={`${id}-help`}
                onClick={() => setHelp((value) => !value)}
              >
                ?
              </button>
              <Button type="button" variant="ghost" onClick={onCancel} disabled={busy}>
                Later
              </Button>
              <Button
                type="submit"
                disabled={
                  busy ||
                  Array.from(passphrase).length < contract.minimumCharacters ||
                  new TextEncoder().encode(passphrase).length >
                    contract.maximumPassphraseBytes
                }
              >
                {busy ? "Unlocking…" : setup ? "Create vault" : "Unlock"}
              </Button>
            </div>
          </form>
        )}
        {help ? (
          <ul id={`${id}-help`} className="vault-details">
            <li>Protected data is encrypted on this device.</li>
            <li>Unlock lasts until Nessa quits or its gateway restarts.</li>
            <li>Nessa cannot recover a forgotten passphrase.</li>
          </ul>
        ) : null}
      </div>
    </Dialog>
  )
}
