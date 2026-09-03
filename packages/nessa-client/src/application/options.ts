import type { ClientInfo, ClientRole, SurfaceInfo } from "../protocol/index.js"

/** Options for establishing a Nessa Client API session. */
export type NessaClientConnectOptions = {
  url?: string
  role: ClientRole
  surface: SurfaceInfo
  client: ClientInfo
  auth: { token: string }
  minProtocol?: number
  maxProtocol?: number
  /** Per-RPC timeout in ms (default 30_000). */
  requestTimeoutMs?: number
}
