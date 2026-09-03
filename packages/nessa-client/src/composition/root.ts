import {
  runConnectHandshake,
  waitForConnectChallenge,
} from "../application/connect-flow.js"
import { NessaClient } from "../presentation/nessa-client.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { waitForSocketOpen, WireSession } from "../transport/index.js"

/**
 * Composition root — wires transport + application flow into `NessaClient`.
 * Single entry for establishing a connection.
 */
export async function connectClient(
  options: NessaClientConnectOptions,
): Promise<NessaClient> {
  const url = options.url ?? NessaClient.defaultUrl
  const socket = new WebSocket(url)
  const wire = new WireSession(socket)

  try {
    await waitForSocketOpen(socket)
    const challenge = await waitForConnectChallenge(wire)
    const hello = await runConnectHandshake(wire, options, challenge)
    return NessaClient.fromSession(wire, hello)
  } catch (error) {
    socket.close()
    throw error
  }
}
