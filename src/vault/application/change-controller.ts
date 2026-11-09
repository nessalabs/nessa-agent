import { VaultRefusal, type VaultFailure, type VaultSource } from "./ports"
export type VaultChangeView =
  | { readonly kind: "closed" }
  | { readonly kind: "form"; readonly busy: boolean; readonly failure?: VaultFailure }
  | { readonly kind: "finished"; readonly failure?: VaultFailure }
/** Explicit change has a Locked success state and never resolves protected-use
 * requests. Unknown delivery becomes recovery guidance without a second write. */
export class VaultChangeController {
  private view: VaultChangeView = { kind: "closed" }
  private listeners = new Set<() => void>()
  private revision = 0
  constructor(private readonly source: VaultSource) {}
  snapshot = () => this.view
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }
  private publish(view: VaultChangeView) {
    this.view = view
    for (const listener of this.listeners) listener()
  }
  open = () => {
    if (this.view.kind === "closed") this.publish({ kind: "form", busy: false })
  }
  close = () => {
    if (this.view.kind === "form" && this.view.busy) return
    this.revision++
    this.publish({ kind: "closed" })
  }
  submit = async (passphrase: string) => {
    if (this.view.kind !== "form" || this.view.busy) return
    const revision = ++this.revision
    this.publish({ kind: "form", busy: true })
    try {
      if ((await this.source.changePassphrase(passphrase)) !== "locked")
        throw new VaultRefusal("vault_change_not_confirmed")
      if (revision === this.revision) this.publish({ kind: "finished" })
    } catch (cause) {
      if (revision !== this.revision) return
      const failure =
        cause instanceof VaultRefusal ? cause.code : "vault_change_not_confirmed"
      switch (failure) {
        case "vault_change_unchanged":
        case "vault_change_unchanged_audit_unavailable":
        case "vault_change_not_confirmed":
        case "vault_change_not_confirmed_audit_unavailable":
        case "vault_changed_audit_unavailable":
          this.publish({ kind: "finished", failure })
          break
        default:
          this.publish({ kind: "form", busy: false, failure })
      }
    }
  }
  dispose() {
    this.revision++
    this.view = { kind: "closed" }
    this.listeners.clear()
  }
}
