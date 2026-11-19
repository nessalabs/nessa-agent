import {
  VaultRefusal,
  type BiometricStatus,
  type VaultSource,
  type VaultFailure,
} from "./ports"
export type VaultDialog =
  | { readonly kind: "closed" }
  | { readonly kind: "checking" }
  | { readonly kind: "unavailable"; readonly failure: VaultFailure }
  | {
      readonly kind: "setup" | "unlock"
      readonly busy: boolean
      readonly failure?: VaultFailure
      /** Touch ID on this device: offered when available, used when saved. */
      readonly biometric?: BiometricStatus
      /** The system's Touch ID prompt is up. */
      readonly fingerprint?: boolean
    }
const none: BiometricStatus = { available: false, saved: false }
/** Coalesces protected-use requests into one dialog. It stores lifecycle only;
 * passphrase input belongs to the transient form and is not retained here. */
export class VaultController {
  private view: VaultDialog = { kind: "closed" }
  private listeners = new Set<() => void>()
  private pending:
    { promise: Promise<boolean>; resolve: (accepted: boolean) => void } | undefined
  private active = true
  private readRevision = 0
  /** The request whose Touch ID prompt has been shown on its own already. */
  private prompted: Promise<boolean> | undefined
  constructor(private readonly source: VaultSource) {}
  snapshot = () => this.view
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }
  private publish(view: VaultDialog) {
    if (!this.active) return
    this.view = view
    for (const listener of this.listeners) listener()
  }
  activate() {
    this.active = true
  }
  ensure = (showLoading = false): Promise<boolean> => {
    if (this.pending) return this.pending.promise
    let resolve!: (accepted: boolean) => void
    const promise = new Promise<boolean>((done) => {
      resolve = done
    })
    this.pending = { promise, resolve }
    void this.read(showLoading)
    return promise
  }
  private async read(showLoading = false) {
    const pending = this.pending
    const revision = ++this.readRevision
    if (!pending) return
    if (showLoading) this.publish({ kind: "checking" })
    try {
      const state = await this.source.status()
      if (!this.active || this.pending !== pending || revision !== this.readRevision)
        return
      if (state === "unlocked") {
        this.finish(true)
        return
      }
      // Read only where Touch ID can exist, so no other source waits on it.
      const biometric = this.source.biometrics ? await this.biometricStatus() : none
      if (!this.active || this.pending !== pending || revision !== this.readRevision)
        return
      const kind = state === "unconfigured" ? "setup" : "unlock"
      this.publish({
        kind,
        busy: false,
        ...(this.source.biometrics ? { biometric } : {}),
      })
      // Asked once on its own per request: a dismissed prompt leaves the
      // form, and the window regaining focus from the prompt must not reopen it.
      if (kind === "unlock" && biometric.saved && this.prompted !== pending.promise) {
        this.prompted = pending.promise
        void this.useBiometric()
      }
    } catch (cause) {
      if (this.active && this.pending === pending && revision === this.readRevision)
        this.publish({ kind: "unavailable", failure: reason(cause) })
    }
  }
  refreshPending = () => {
    if (
      this.pending &&
      !((this.view.kind === "setup" || this.view.kind === "unlock") && this.view.busy)
    )
      void this.read()
  }
  retry = () => {
    if (this.pending) void this.read(true)
  }
  cancel = () => {
    if ((this.view.kind === "setup" || this.view.kind === "unlock") && this.view.busy)
      return
    this.finish(false)
  }
  private finish(accepted: boolean) {
    const pending = this.pending
    this.pending = undefined
    this.publish({ kind: "closed" })
    pending?.resolve(accepted)
  }
  private async biometricStatus(): Promise<BiometricStatus> {
    try {
      return (await this.source.biometrics?.status()) ?? none
    } catch {
      return none
    }
  }
  /** Touch ID instead of typing: the host shows its prompt and unlocks with
   * the passphrase it kept. Any refusal leaves the passphrase form. */
  useBiometric = async () => {
    const view = this.view
    const pending = this.pending
    const biometrics = this.source.biometrics
    if (view.kind !== "unlock" || view.busy || !view.biometric?.saved || !biometrics)
      return
    const revision = ++this.readRevision
    this.publish({ ...view, busy: true, fingerprint: true, failure: undefined })
    try {
      const state = await biometrics.unlock()
      if (!this.active || this.pending !== pending) return
      if (state !== "unlocked") throw new VaultRefusal("vault_unavailable")
      this.finish(true)
    } catch (cause) {
      if (!this.active || this.pending !== pending) return
      const failure = reason(cause)
      const biometric = await this.biometricStatus()
      if (!this.active || this.pending !== pending || revision !== this.readRevision)
        return
      this.publish({
        kind: "unlock",
        busy: false,
        biometric,
        ...(failure === "biometric_cancelled" ? {} : { failure }),
      })
    }
  }
  submit = async (passphrase: string, remember = false) => {
    const view = this.view
    const pending = this.pending
    if ((view.kind !== "setup" && view.kind !== "unlock") || view.busy) return
    const revision = ++this.readRevision
    this.publish({ ...view, busy: true, failure: undefined })
    try {
      const state = await this.source[view.kind](
        passphrase,
        remember && view.biometric?.available === true,
      )
      if (!this.active || this.pending !== pending) return
      if (state !== "unlocked") throw new VaultRefusal("vault_unavailable")
      this.finish(true)
    } catch (cause) {
      if (!this.active || this.pending !== pending) return
      const failure = reason(cause)
      let kind = view.kind
      try {
        if ((await this.source.status()) === "locked") kind = "unlock"
      } catch {
        /* The original refusal remains useful if status is unavailable. */
      }
      if (!this.active || this.pending !== pending || revision !== this.readRevision)
        return
      this.publish({ kind, busy: false, failure, biometric: view.biometric })
    }
  }
  dispose() {
    this.finish(false)
    this.active = false
    this.listeners.clear()
  }
}
function reason(cause: unknown): VaultFailure {
  return cause instanceof VaultRefusal ? cause.code : "vault_unavailable"
}
