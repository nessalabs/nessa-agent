import {
  NessaClient,
  NessaRpcError,
  type HealthResult,
  type ProductSessionReady,
  type CredentialSource,
  type GatewayEndpointSource,
  type Stage,
  type SurfaceKind,
} from "@nessa/client"

import { host } from "../../../host"
import { browserSessionUrl } from "./browser-auth"

export type EstablishedDevSession = {
  client: NessaClient
  hello: ProductSessionReady
  health: HealthResult
}

/** Thrown when connect succeeded but a post-connect probe failed. */
export class SessionHealthError extends Error {
  readonly cause: unknown

  constructor(message: string, cause?: unknown) {
    super(message)
    this.name = "SessionHealthError"
    this.cause = cause
  }
}

export type ConnectDevSessionDeps = {
  connect?: typeof NessaClient.connect
  credentialSource?: CredentialSource
  endpointSource?: GatewayEndpointSource
  stage?: Stage
  clientId?: string
  /** Which surface this is, for the gateway's record; the panel when not said. */
  surfaceKind?: SurfaceKind
  browserUrl?: string
  gatewayBaseUrl?: string
}

function gatewaySessionUrl(baseUrl: string): string {
  const url = new URL(baseUrl)
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:"
  return url.origin
}

/** Authenticate the chat surface and verify authorized gateway health. */
export async function connectDevSession(
  deps: ConnectDevSessionDeps = {},
): Promise<EstablishedDevSession> {
  const connect = deps.connect ?? NessaClient.connect.bind(NessaClient)
  const stage = deps.stage ?? "dev"
  const client = await connect({
    profile: "product",
    stage,
    ...(deps.browserUrl
      ? { url: deps.browserUrl, auth: { browserCookie: true as const } }
      : deps.gatewayBaseUrl
        ? { url: gatewaySessionUrl(deps.gatewayBaseUrl) }
        : { endpointSource: deps.endpointSource }),
    credentialSource: deps.credentialSource,
    role: "surface",
    surface: { kind: deps.surfaceKind ?? "panel", instance: crypto.randomUUID() },
    client: {
      id: deps.clientId ?? "nessa-panel",
      version: "0.1.0",
      platform: host.kind,
    },
  })
  try {
    const health = await client.server.health()
    return { client, hello: client.productSession, health }
  } catch (error) {
    client.close()
    throw new SessionHealthError(
      error instanceof Error ? error.message : "Session probe failed",
      error,
    )
  }
}

/**
 * Connects a surface running in a browser, over the gateway session this
 * origin signed in to: the one way a browser surface connects, whichever it
 * is. An origin that is not signed in is refused as `unauthorized` before
 * anything is opened, as an expired session is.
 */
export async function connectBrowserSession(deps: {
  auth: { restore(): Promise<boolean> }
  stage: Stage
  clientId: string
  /** The page's own URL: the session is this origin's. */
  pageUrl: string
  surfaceKind?: SurfaceKind
  connect?: typeof NessaClient.connect
}): Promise<EstablishedDevSession> {
  if (!(await deps.auth.restore()))
    throw new NessaRpcError("unauthorized", "Please sign in again.")
  return connectDevSession({
    connect: deps.connect,
    stage: deps.stage,
    clientId: deps.clientId,
    surfaceKind: deps.surfaceKind,
    browserUrl: browserSessionUrl(deps.pageUrl, deps.stage),
  })
}
