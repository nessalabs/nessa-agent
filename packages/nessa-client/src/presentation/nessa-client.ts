import type { ProductSessionReady } from "../protocol/product-types.js"
import type { ManagedSession, ConnectionState } from "../application/managed-session.js"
import type { EventHandler, NessaClientEvents } from "../application/events.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { establishManagedSession } from "../composition/root.js"
import type {
  AttachmentUploadTransport,
  UploadTimer,
} from "../application/attachment-upload.js"
import { createAttachmentApi, type AttachmentApi } from "./attachment-api.js"
import { createConversationApi, type ConversationApi } from "./conversation-api.js"
import { createServerApi, type ServerApi } from "./server-api.js"
import { createCredentialApi, type CredentialApi } from "./credential-api.js"
import { createAuthApi, type AuthApi } from "./auth-api.js"

/**
 * Entry point for a connection to the Nessa gateway. Create one with {@link connect},
 * then use its API namespaces to make typed requests over the same WebSocket.
 *
 * Composition validates options and opens the transport. The product handshake
 * checks protocol compatibility before sending credentials. A managed session
 * replaces the transport after eligible failures while retaining API objects and
 * subscriptions. Requests are never queued during recovery or automatically replayed.
 *
 * Call {@link close} when the owning surface is disposed.
 * See {@link NessaClientConfig} for timeout and recovery settings.
 *
 * @example
 * ```ts
 * // After connecting with product options:
 * const health = await client.server.health();
 * const identity = await client.auth.session();
 * client.close();
 * ```
 */
export class NessaClient {
  /**
   * Default development gateway address. Non-development stages require an
   * explicit URL.
   *
   * 7421 is the `dev` port in `protocol/defaults/gateway-ports.json`, not 7420:
   * an installed Nessa owns 7420 through a launchd service that outlives the
   * app, so the dev gateway listens next door rather than fighting it for the
   * socket. This literal is checked against that table in `nessa-client.test.ts`
   * — the package stays self-contained, and the number still cannot drift.
   *
   * When `url` is omitted, local Node clients first verify the gateway's private
   * endpoint publication. This value remains the dev-only fallback when that
   * publication is absent or unreadable.
   */
  static readonly defaultUrl = "ws://127.0.0.1:7421"

  /** Authorized gateway health. */
  readonly server: ServerApi
  /** Authorized agent conversations, live views, and lifecycle controls. */
  readonly conversation: ConversationApi
  /** Stage files into a conversation so a message can refer to them by digest. */
  readonly attachments: AttachmentApi
  /** Issue, list, and revoke scoped product credentials, subject to server authorization. */
  readonly credentials: CredentialApi
  /** Fetch a fresh snapshot of the authenticated product identity and restrictions. */
  readonly auth: AuthApi

  private constructor(
    private readonly wire: ManagedSession,
    /** Authenticated connection profile. */
    readonly profile: "product",
    newRequestId: () => string,
    upload: AttachmentUploadTransport,
    uploadTimer: UploadTimer,
  ) {
    this.server = createServerApi(wire)
    this.conversation = createConversationApi(wire, newRequestId)
    this.attachments = createAttachmentApi(wire, upload, newRequestId, uploadTimer)
    this.credentials = createCredentialApi(wire, newRequestId)
    this.auth = createAuthApi(wire)
  }

  /** Validate options, authenticate, and resolve only when a session is ready.
   * Product setup retries eligible transient failures according to config.retry.
   * Authentication and version rejection stop immediately.
   * @param options - Profile, caller metadata, credentials, and optional connection tuning.
   * @returns A connected client that owns its transport and recovery lifecycle.
   * @throws StageConfigError for invalid options, NessaProtocolCompatibilityError for
   * incompatible versions, or an RPC/transport error when setup fails. */
  static async connect(options: NessaClientConnectOptions): Promise<NessaClient> {
    const { managed, profile, newRequestId, upload, uploadTimer } =
      await establishManagedSession(options, NessaClient.defaultUrl)
    return new NessaClient(managed, profile, newRequestId, upload, uploadTimer)
  }

  /** Current authenticated handshake snapshot; available only while connected. */
  get productSession(): ProductSessionReady {
    const ready = this.wire.ready
    if (!ready) throw new Error("session is not connected")
    return ready
  }

  /** Subscribe to a typed protocol event. Returns an unsubscribe function. The subscription survives transient recovery, but missed events are not replayed. */
  on<K extends keyof NessaClientEvents>(event: K, handler: EventHandler<K>): () => void {
    return this.wire.onEvent(event, handler as (payload: unknown) => void)
  }

  /** Current lifecycle state. Subscriptions survive transient reconnection. */
  get connectionState(): ConnectionState {
    return this.wire.state
  }

  /** Observe subsequent lifecycle transitions. Read connectionState for the initial snapshot. Returns an unsubscribe function. */
  onConnectionStateChange(handler: (state: ConnectionState) => void): () => void {
    return this.wire.onState(handler)
  }

  /** Observe permanent closure, including explicit close or retry exhaustion. Late subscribers receive the final error immediately. Returns an unsubscribe function. */
  onClose(handler: (error: Error) => void): () => void {
    return this.wire.onClose(handler)
  }

  /** Permanently close this client, cancel recovery, reject pending requests, and release subscriptions. Safe to call repeatedly. Create a new client to connect again. */
  close(): void {
    this.wire.close()
  }
}
