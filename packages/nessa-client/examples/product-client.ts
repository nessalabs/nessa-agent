import {
  NessaClient,
  NessaClientConfig,
  ClientRole,
  ClientPlatform,
  SurfaceKind,
  ConnectionProfile,
  Stage,
} from "../src/index.js"

/** Caller loads credential evidence from its own private storage. */
export async function checkHealth(credential: string) {
  const client = await NessaClient.connect({
    profile: ConnectionProfile.Product,
    stage: Stage.Dev,
    role: ClientRole.Surface,
    surface: { kind: SurfaceKind.Cli, instance: "terminal" },
    client: { id: "terminal", version: "1.0.0", platform: ClientPlatform.Node },
    auth: { credential },
    config: new NessaClientConfig(),
  })
  try {
    return await client.server.health()
  } finally {
    client.close()
  }
}
