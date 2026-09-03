import {
  runConnectHandshake,
  waitForConnectChallenge,
} from "../application/connect-flow.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import type { HelloOk } from "../protocol/index.js"
import { waitForSocketOpen, WireSession } from "../transport/index.js"

export type EstablishedSession = {
  wire: WireSession
  hello: HelloOk
}

/**
 * Composition root — wires transport + application flow into a session.
 * Presentation wraps this into `NessaClient` (avoids an import cycle).
 */
export async function establishSession(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): Promise<EstablishedSession> {
  const url = options.url ?? defaultUrl
  const socket = new WebSocket(url)
  const wire = new WireSession(socket, {
    requestTimeoutMs: options.requestTimeoutMs,
  })

  // Subscribe before open so a fast challenge cannot race past the listener.
  const challengePromise = waitForConnectChallenge(wire)
  // If open fails we close the socket (rejecting challengePromise). Attach a
  // no-op catch so that rejection cannot become an unhandled rejection before
  // we await it on the success path.
  challengePromise.catch(() => {})

  try {
    await waitForSocketOpen(socket)
    const challenge = await challengePromise
    const hello = await runConnectHandshake(wire, options, challenge)
    return { wire, hello }
  } catch (error) {
    socket.close()
    throw error
  }
}
