import type { CredentialSource } from "./credential-source.js"
import type { NessaClientConfig } from "./client-config.js"
import type { ClientInfo, ClientRole, SurfaceInfo } from "../protocol/index.js"
import type { Stage } from "./stage.js"

/** Options for establishing a Nessa Client API session. */
export type CommonConnectOptions = {
  /**
   * Deployment stage, default dev. Product credentials are always required.
   * In dev, URL defaults to loopback. Other stages require an explicit URL;
   * non-loopback URLs outside dev require wss. Authentication is required in every stage.
   */
  stage?: Stage
  /** Gateway WebSocket URL. Defaults to loopback in dev; product sessions use the /session endpoint. */
  url?: string
  /** Caller role metadata; does not confer authorization. */
  role: ClientRole
  /** Surface metadata; identity and permissions come from the credential. */
  surface: SurfaceInfo
  /** Caller metadata. The handshake sends its id; other fields describe the local host. */
  client: ClientInfo
  /** Minimum accepted protocol version. Default 1. */
  minProtocol?: number
  /** Maximum accepted protocol version. Default 1. */
  maxProtocol?: number
  /** Validated retry and timeout settings. Omit for sensible defaults. */
  config?: NessaClientConfig
}

/**
 * Mandatory-authentication product profile served at `/session`.
 * Connection setup retries transient transport failures up to three attempts by default,
 * with a fresh socket/challenge each time. Authentication and protocol rejection
 * are not retried. Established-session RPCs are never automatically replayed.
 */
export type ProductConnectOptions = CommonConnectOptions & {
  profile?: "product"
  /** Opaque credential evidence. It is sent only in `session.authenticate`. */
  auth?: { credential: string }
  /** Host storage override. Node otherwise loads the assigned private surface file. */
  credentialSource?: CredentialSource
}

/** Options for establishing an authenticated gateway session. */
export type NessaClientConnectOptions = ProductConnectOptions
