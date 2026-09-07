import { NessaProtocolCompatibilityError } from "./protocol-compatibility-error.js"
import { RetryableConnectError } from "./connect-retry.js"
import type { ResolvedProductConnectOptions } from "./resolve-options.js"
import {
  assertProductSessionReady,
  assertSessionChallenge,
} from "../protocol/validate.js"
import {
  ProductEvent,
  ProductMethod,
  type ProductSessionReady,
  type SessionChallenge,
} from "../protocol/product-types.js"
import type { WireSession } from "../transport/wire-session.js"

const CHALLENGE_TIMEOUT_MS = 5_000

export function waitForSessionChallenge(session: WireSession): Promise<SessionChallenge> {
  return new Promise((resolve, reject) => {
    let settled = false
    const settle = (fn: () => void) => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      offEvent()
      offClose()
      fn()
    }
    const timeout = setTimeout(
      () => settle(() => reject(new RetryableConnectError("session.challenge timeout"))),
      CHALLENGE_TIMEOUT_MS,
    )
    const offEvent = session.onEvent(ProductEvent.SessionChallenge, (payload) => {
      try {
        const challenge = assertSessionChallenge(payload)
        settle(() => resolve(challenge))
      } catch (error) {
        settle(() =>
          reject(error instanceof Error ? error : new Error("invalid session.challenge")),
        )
      }
    })
    const offClose = session.onClose((error) => settle(() => reject(error)))
  })
}

export async function runProductHandshake(
  session: WireSession,
  options: ResolvedProductConnectOptions,
  challenge: SessionChallenge,
): Promise<ProductSessionReady> {
  const minVersion = options.minProtocol ?? 1
  const maxVersion = options.maxProtocol ?? 1
  if (maxVersion < challenge.minVersion || minVersion > challenge.maxVersion) {
    throw new NessaProtocolCompatibilityError(
      minVersion,
      maxVersion,
      challenge.minVersion,
      challenge.maxVersion,
    )
  }
  const remainingMs = challenge.expiresAt * 1_000 - Date.now()
  if (remainingMs <= 0) {
    throw new RetryableConnectError("session.challenge expired")
  }
  const payload = await session.request(
    ProductMethod.SessionAuthenticate,
    {
      minVersion,
      maxVersion,
      nonce: challenge.nonce,
      credential: options.auth.credential,
      client: { id: options.client.id },
    },
    Math.min(options.config.requestTimeoutMs, remainingMs),
  )
  return assertProductSessionReady(payload)
}
