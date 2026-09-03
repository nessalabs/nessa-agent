import type { ClientInfo, ClientRole, SurfaceInfo } from "../protocol/index.js"
import type { Stage } from "./stage.js"

/** Options for establishing a Nessa Client API session. */
export type NessaClientConnectOptions = {
  /**
   * Deployment stage. Defaults to `"dev"`.
   * In `dev`, `url` and `auth.token` may be omitted (loopback + `dev-token`).
   * Other stages require both.
   */
  stage?: Stage
  url?: string
  role: ClientRole
  surface: SurfaceInfo
  client: ClientInfo
  /** Required outside `dev`. In `dev`, defaults to `dev-token` when omitted. */
  auth?: { token: string }
  minProtocol?: number
  maxProtocol?: number
  /** Per-RPC timeout in ms (default 30_000). */
  requestTimeoutMs?: number
}
