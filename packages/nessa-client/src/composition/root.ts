import { LocalFileCredentialSource } from "../transport/local-credential-source.js"
import { NessaCredentialUnavailableError } from "../application/credential-source.js"
import { ManagedSession } from "../application/managed-session.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { retryProductConnection } from "../application/connect-retry.js"
import {
  resolveConnectOptions,
  type ResolvedConnectOptions,
} from "../application/resolve-options.js"
import {
  runProductHandshake,
  waitForSessionChallenge,
} from "../application/product-connect-flow.js"
import type { ProductSessionReady } from "../protocol/product-types.js"
import { waitForSocketOpen, WireSession } from "../transport/index.js"

export type EstablishedSession = {
  wire: WireSession
  ready: ProductSessionReady
  profile: "product"
}

/**
 * Composition root — wires transport + application flow into a session.
 * Presentation wraps this into `NessaClient` (avoids an import cycle).
 */
export async function establishSession(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): Promise<EstablishedSession> {
  const resolved = resolveConnectOptions(
    await loadCredential(options, defaultUrl),
    defaultUrl,
  )
  return retryProductConnection(
    () => establishAttempt(resolved),
    {
      wait: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
      random: Math.random,
    },
    resolved.config.retry,
  )
}

async function establishAttempt(
  resolved: ResolvedConnectOptions,
  signal?: AbortSignal,
): Promise<EstablishedSession> {
  signal?.throwIfAborted()
  const socket = new WebSocket(resolved.url)
  const wire = new WireSession(socket, {
    requestTimeoutMs: resolved.config.requestTimeoutMs,
  })

  // Subscribe before open so a fast challenge cannot race past the listener.
  const profile = resolved.profile
  const challengePromise = waitForSessionChallenge(wire)
  // If open fails we close the socket (rejecting challengePromise). Attach a
  // no-op catch so that rejection cannot become an unhandled rejection before
  // we await it on the success path.
  challengePromise.catch(() => {})

  const opening = new AbortController()
  const abort = () => {
    opening.abort()
    wire.close()
  }
  signal?.addEventListener("abort", abort, { once: true })
  try {
    const [, challenge] = await Promise.all([
      waitForSocketOpen(socket, undefined, opening.signal),
      challengePromise,
    ])
    const ready = await runProductHandshake(wire, resolved, challenge)
    signal?.throwIfAborted()
    if (wire.termination) throw wire.termination
    return { wire, ready, profile }
  } catch (error) {
    wire.close()
    throw error
  } finally {
    signal?.removeEventListener("abort", abort)
    opening.abort()
  }
}

/** Compose the persistent facade after initial authentication succeeds. */
export async function establishManagedSession(
  options: NessaClientConnectOptions,
  defaultUrl: string,
) {
  const prepared = await loadCredential(options, defaultUrl)
  const resolved = resolveConnectOptions(prepared, defaultUrl)
  const initial = await establishSession(prepared, defaultUrl)
  const managed = new ManagedSession(
    initial,
    resolved.config,
    (signal) => establishAttempt(resolved, signal),
    {
      random: Math.random,
      wait: (ms, signal) =>
        new Promise<void>((resolve, reject) => {
          if (signal.aborted) {
            reject(signal.reason)
            return
          }
          const abort = () => {
            clearTimeout(timer)
            reject(signal.reason)
          }
          const timer = setTimeout(() => {
            signal.removeEventListener("abort", abort)
            resolve()
          }, ms)
          signal.addEventListener("abort", abort, { once: true })
        }),
    },
  )
  return { managed, profile: initial.profile, newRequestId: () => crypto.randomUUID() }
}

async function loadCredential(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): Promise<NessaClientConnectOptions> {
  if (options.auth !== undefined) return options
  const stage = options.stage ?? "dev"
  const url = options.url ?? (stage === "dev" ? defaultUrl : undefined)
  if (!url)
    throw new NessaCredentialUnavailableError(
      "An explicit gateway URL is required outside dev",
    )
  let source = options.credentialSource
  if (!source && typeof process !== "undefined" && process.versions?.node) {
    source = new LocalFileCredentialSource({
      home: process.platform === "win32" ? process.env.USERPROFILE : process.env.HOME,
      dataDir: process.env.NESSA_DATA_DIR,
      instance: process.env.NESSA_INSTANCE,
      uid: process.getuid?.(),
    })
  }
  if (!source)
    throw new NessaCredentialUnavailableError(
      "This host must supply a trusted credential source",
    )
  return {
    ...options,
    auth: { credential: await source.load({ clientId: options.client.id, stage, url }) },
  }
}
