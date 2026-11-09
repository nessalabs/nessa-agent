import { VaultRefusal, type VaultSource, type VaultFailure } from "./ports"
export type VaultDialog =
  | { readonly kind: "closed" }
  | { readonly kind: "checking" }
  | { readonly kind: "unavailable"; readonly failure: VaultFailure }
  | {
      readonly kind: "setup" | "unlock"
      readonly busy: boolean
      readonly failure?: VaultFailure
    }
/** Coalesces protected-use requests into one dialog. It stores lifecycle only;
 * passphrase input belongs to the transient form and is not retained here. */
export class VaultController {
  private view: VaultDialog = { kind: "closed" }
  private listeners = new Set<() => void>()
  private pending:
    { promise: Promise<boolean>; resolve: (accepted: boolean) => void } | undefined
  private active = true
  private readRevision = 0
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
      } else {
        this.publish({ kind: state === "unconfigured" ? "setup" : "unlock", busy: false })
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
  submit = async (passphrase: string) => {
    const view = this.view
    const pending = this.pending
    if ((view.kind !== "setup" && view.kind !== "unlock") || view.busy) return
    const revision = ++this.readRevision
    this.publish({ ...view, busy: true, failure: undefined })
    try {
      const state = await this.source[view.kind](passphrase)
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
      this.publish({ kind, busy: false, failure })
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
