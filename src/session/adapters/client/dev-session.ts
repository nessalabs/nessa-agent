import { NessaClient, type HealthResult, type HelloOk } from "@nessa/client"

import { host } from "../../../host"

export type EstablishedDevSession = {
  client: NessaClient
  hello: HelloOk
  health: HealthResult
}

/** Thrown when connect succeeded but the post-connect health probe failed. */
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
}

/**
 * Open a stage=dev session against the local nessa-server defaults
 * (`ws://127.0.0.1:7420`, `dev-token`), then probe `server.health`.
 *
 * Closes the socket if health fails so callers never see a leaked session.
 */
export async function connectDevSession(
  deps: ConnectDevSessionDeps = {},
): Promise<EstablishedDevSession> {
  const connect = deps.connect ?? NessaClient.connect.bind(NessaClient)
  const client = await connect({
    stage: "dev",
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
    return { client, hello: client.session, health }
  } catch (error) {
    client.close()
    throw new SessionHealthError(
      error instanceof Error ? error.message : "Health check failed",
      error,
    )
  }
}
