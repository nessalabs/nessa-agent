import { NessaClient, type HealthResult, type HelloOk } from "@nessa/client"

import { host } from "../../../host"

export type EstablishedDevSession = {
  client: NessaClient
  hello: HelloOk
  health: HealthResult
}

/**
 * Open a stage=dev session against the local nessa-server defaults
 * (`ws://127.0.0.1:7420`, `dev-token`).
 */
export async function connectDevSession(): Promise<EstablishedDevSession> {
  const client = await NessaClient.connect({
    stage: "dev",
    role: "surface",
    surface: { kind: "panel", instance: crypto.randomUUID() },
    client: {
      id: "nessa-panel",
      version: "0.1.0",
      platform: host.kind,
    },
  })
  const health = await client.server.health()
  return { client, hello: client.session, health }
}
