import {
  assertConnectChallenge,
  assertHelloOk,
  type ConnectChallenge,
  type ConnectParams,
  type HelloOk,
} from "../protocol/index.js"
import { Event, Method } from "../generated/catalog.js"
import type { WireSession } from "../transport/wire-session.js"
import type { ResolvedConnectOptions } from "./resolve-options.js"

const CHALLENGE_TIMEOUT_MS = 5_000

/** Wait for the server `connect.challenge` event before sending `connect`. */
export function waitForConnectChallenge(session: WireSession): Promise<ConnectChallenge> {
  return new Promise((resolve, reject) => {
    let settled = false

    const settle = (fn: () => void) => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      off()
      offClose()
      fn()
    }

    const timeout = setTimeout(
      () => settle(() => reject(new Error("connect.challenge timeout"))),
      CHALLENGE_TIMEOUT_MS,
    )

    const off = session.onEvent(Event.ConnectChallenge, (payload) => {
      try {
        const challenge = assertConnectChallenge(payload)
        settle(() => resolve(challenge))
      } catch (error) {
        settle(() =>
          reject(error instanceof Error ? error : new Error("invalid connect.challenge")),
        )
      }
    })

    const offClose = session.onClose(() => {
      settle(() => reject(new Error("WebSocket closed before connect.challenge")))
    })
  })
}

/** Run the `connect` RPC and return the validated hello payload. */
export async function runConnectHandshake(
  session: WireSession,
  options: ResolvedConnectOptions,
  challenge: ConnectChallenge,
): Promise<HelloOk> {
  const params: ConnectParams = {
    minProtocol: options.minProtocol ?? 1,
    maxProtocol: options.maxProtocol ?? 1,
    role: options.role,
    surface: options.surface,
    client: options.client,
    auth: {
      token: options.auth.token,
      nonce: challenge.nonce,
    },
  }

  const payload = await session.request(Method.Connect, params)
  return assertHelloOk(payload)
}
