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

/** Follow one submitted turn using the existing connected client and release the stream. */
export async function runThread(client: NessaClient) {
  const { conversationId: threadId } = await client.createThread()
  const events = await client.subscribe(threadId)
  try {
    const receipt = await client.sendMessage(threadId, "Check the build")
    for await (const batch of events) {
      if (
        batch.events.some(
          (event) =>
            event.kind === "turn.ended" && event.executionId === receipt.executionId,
        )
      )
        return batch.cursor
    }
    return undefined
  } finally {
    await events.close()
  }
}
