import {
  NessaClient,
  type HealthResult,
  type ProductSessionReady,
  type CredentialSource,
  type Stage,
} from "@nessa/client"

import { gatewayPort } from "../../../env/gateway-ports"
import { host } from "../../../host"

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
  stage?: Stage
  clientId?: string
  browserUrl?: string
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
    // Without a proxied browser URL this talks to the gateway directly, so it
    // asks the one table where this stage listens.
    url: deps.browserUrl ?? `ws://127.0.0.1:${gatewayPort(stage)}/session`,
    ...(deps.browserUrl ? { auth: { browserCookie: true as const } } : {}),
    credentialSource: deps.credentialSource,
    role: "surface",
    surface: { kind: "panel", instance: crypto.randomUUID() },
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
