import { NessaClient, type HealthResult, type HelloOk, type PingResult } from "@nessa/client"

import { host } from "../../../host"

export type EstablishedDevSession = {
  client: NessaClient
  hello: HelloOk
  health: HealthResult
  ping: PingResult
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
}

/**
 * Open a stage=dev session against the local nessa-server defaults
 * (`ws://127.0.0.1:7420`, `dev-token`), then probe `server.health` and
 * `server.ping` (ADR 0006) through `NessaClient`.
 *
 * Closes the socket if a probe fails so callers never see a leaked session.
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
    const nonce = crypto.randomUUID()
    const ping = await client.server.ping(nonce)
    if (ping.nonce !== nonce) {
      throw new Error("ping nonce mismatch")
    }
    return { client, hello: client.session, health, ping }
  } catch (error) {
    client.close()
    throw new SessionHealthError(
      error instanceof Error ? error.message : "Session probe failed",
      error,
    )
  }
}
