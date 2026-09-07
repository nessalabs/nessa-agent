import {
  NessaClient,
  type HealthResult,
  type ProductSessionReady,
  type CredentialSource,
  type Stage,
} from "@nessa/client"

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
}

/** Authenticate the chat surface and verify authorized gateway health. */
export async function connectDevSession(
  deps: ConnectDevSessionDeps = {},
): Promise<EstablishedDevSession> {
  const connect = deps.connect ?? NessaClient.connect.bind(NessaClient)
  const client = await connect({
    profile: "product",
    stage: deps.stage ?? "dev",
    url: "ws://127.0.0.1:7420/session",
    credentialSource: deps.credentialSource,
    role: "surface",
    surface: { kind: "panel", instance: crypto.randomUUID() },
    client: {
      id: "nessa-panel",
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
