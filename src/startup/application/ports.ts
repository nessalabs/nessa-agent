/** Where a starting gateway is. The host reports it (ADR 221). */
export type StartupStep = "preparing" | "replacing" | "launching"

/** The host-owned progress of the gateway reconciliation for this launch. */
export type GatewayStartup =
  | { readonly revision: number; readonly state: "unmanaged" }
  | { readonly revision: number; readonly state: "starting"; readonly step: StartupStep }
  | { readonly revision: number; readonly state: "ready" }
  | { readonly revision: number; readonly state: "failed"; readonly message: string }

/** Where a surface observes and retries the native gateway lifecycle. */
export interface GatewayStartupSource {
  snapshot(): Promise<GatewayStartup>
  subscribe(handler: (startup: GatewayStartup) => void): Promise<() => void>
  retry(): Promise<void>
}

/**
 * Whether the host could put itself together at launch (ADR 221). `refused`
 * means nothing else the host offers will answer; `details` is the technical
 * reason, kept behind "Details" for whoever helps the person.
 */
export type HostStartup =
  { readonly state: "ready" } | { readonly state: "refused"; readonly details: string }
