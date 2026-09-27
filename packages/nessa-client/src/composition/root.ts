import { LocalFileCredentialSource } from "../transport/local-credential-source.js"
import { NessaCredentialUnavailableError } from "../application/credential-source.js"
import { ManagedSession } from "../application/managed-session.js"
import { NessaClientConfig } from "../application/client-config.js"
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
import { attachmentUploadUrl } from "../application/attachment-upload.js"
import type { AttachmentUploadRequest } from "../application/attachment-upload.js"
import { fetchAttachmentUpload } from "../transport/attachment-upload.js"
import { waitForSocketOpen, WireSession } from "../transport/index.js"
import { nodeGatewayEndpointSource } from "../transport/local-gateway-endpoint.js"

export type EstablishedSession = {
  wire: WireSession
  ready: ProductSessionReady
  profile: "product"
  url: string
}

/**
 * Composition root — wires transport + application flow into a session.
 * Presentation wraps this into `NessaClient` (avoids an import cycle).
 */
export async function establishSession(
  options: NessaClientConnectOptions,
  defaultUrl: string,
): Promise<EstablishedSession> {
  const prepared = await composeLocalSources(options)
  const config = prepared.config ?? new NessaClientConfig()
  return retryProductConnection(
    async () => (await establishPreparedAttempt(prepared, defaultUrl)).session,
    {
      wait: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
      random: Math.random,
    },
    config.retry,
  )
}

type PreparedAttempt = {
  session: EstablishedSession
  resolved: ResolvedConnectOptions
}

async function establishPreparedAttempt(
  options: NessaClientConnectOptions,
  defaultUrl: string,
  signal?: AbortSignal,
): Promise<PreparedAttempt> {
  signal?.throwIfAborted()
  const located = await locateEndpoint(options)
  signal?.throwIfAborted()
  const resolved = resolveConnectOptions(
    await loadCredential(located, defaultUrl),
    defaultUrl,
  )
  return { session: await establishAttempt(resolved, signal), resolved }
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
    return { wire, ready, profile, url: resolved.url }
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
  const prepared = await composeLocalSources(options)
  const config = prepared.config ?? new NessaClientConfig()
  const initial = await retryProductConnection(
    () => establishPreparedAttempt(prepared, defaultUrl),
    {
      wait: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
      random: Math.random,
    },
    config.retry,
  )
  const managed = new ManagedSession(
    initial.session,
    config,
    async (signal) =>
      (await establishPreparedAttempt(prepared, defaultUrl, signal)).session,
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
  return {
    managed,
    profile: initial.session.profile,
    newRequestId: () => crypto.randomUUID(),
    // Uploads go to the gateway this session is connected to, and nowhere a
    // caller names later. `fetch` is looked up per request, so a runtime
    // without one fails at the upload, as a typed `unreachable`.
    upload: {
      put(request: AttachmentUploadRequest) {
        const url = managed.url
        if (!url) return Promise.reject(new Error("Session is not connected"))
        return fetchAttachmentUpload(attachmentUploadUrl(url), (target, init) =>
          globalThis.fetch(target, init),
        ).put(request)
      },
    },
    // The real clock for an upload's deadline; tests of the API pass their own.
    uploadTimer: (ms: number, elapsed: () => void) => {
      const timer = setTimeout(elapsed, ms)
      return () => clearTimeout(timer)
    },
  }
}

async function locateEndpoint(
  options: NessaClientConnectOptions,
): Promise<NessaClientConnectOptions> {
  if (options.url !== undefined) return options
  const url = await options.endpointSource?.load({ stage: options.stage ?? "dev" })
  return url === undefined ? options : { ...options, url }
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
  const source = options.credentialSource
  if (!source)
    throw new NessaCredentialUnavailableError(
      "This host must supply a trusted credential source",
    )
  return {
    ...options,
    auth: { credential: await source.load({ clientId: options.client.id, stage, url }) },
  }
}

async function composeLocalSources(
  options: NessaClientConnectOptions,
): Promise<NessaClientConnectOptions> {
  if (typeof process === "undefined" || !process.versions?.node) return options
  const namespace = {
    home: process.platform === "win32" ? process.env.USERPROFILE : process.env.HOME,
    dataDir: process.env.NESSA_DATA_DIR,
    instance: process.env.NESSA_INSTANCE,
    uid: process.getuid?.(),
  }
  return {
    ...options,
    endpointSource:
      options.url === undefined
        ? (options.endpointSource ?? (await nodeGatewayEndpointSource(namespace)))
        : options.endpointSource,
    credentialSource:
      options.auth === undefined
        ? (options.credentialSource ?? new LocalFileCredentialSource(namespace))
        : options.credentialSource,
  }
}
