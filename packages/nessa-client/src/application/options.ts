import type { ClientInfo, ClientRole, SurfaceInfo } from "../protocol/index.js"
import type { Stage } from "./stage.js"

/** Options for establishing a Nessa Client API session. */
export type NessaClientConnectOptions = {
  /**
   * Deployment stage. Defaults to `"dev"`.
   * In `dev`, `url` may be omitted (loopback default) and `auth.token` may be
   * omitted **only** for loopback URLs (default `dev-token`). Non-loopback
   * URLs always need an explicit token. Other stages require both `url` and
   * `auth.token`; non-loopback non-dev URLs must use `wss:`.
   */
  stage?: Stage
  url?: string
  role: ClientRole
  surface: SurfaceInfo
  client: ClientInfo
  /**
   * Required outside `dev`, and for any non-loopback URL.
   * In `dev` on loopback, defaults to `dev-token` when omitted.
   */
  auth?: { token: string }
  minProtocol?: number
  maxProtocol?: number
  /** Per-RPC timeout in ms (default 30_000). */
  requestTimeoutMs?: number
}
