import { createSubscriptionApi, type SubscriptionApi } from "./subscription-api.js"
import type { ProductSessionReady } from "../protocol/product-types.js"
import type { ManagedSession, ConnectionState } from "../application/managed-session.js"
import type { EventHandler, NessaClientEvents } from "../application/events.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { establishManagedSession } from "../composition/root.js"
import type { AttachmentUploadTransport } from "../application/attachment-upload.js"
import type { RequestTimer } from "../application/gateway-http.js"
import type { McpResourceTransport } from "../application/mcp-resource-fetch.js"
import { createAttachmentApi, type AttachmentApi } from "./attachment-api.js"
import { createMcpAppsApi, type McpAppsApi } from "./mcp-apps-api.js"
import { createMcpServersApi, type McpServersApi } from "./mcp-servers-api.js"
import { createConversationApi, type ConversationApi } from "./conversation-api.js"
import { createAgentsApi, type AgentsApi } from "./agents-api.js"
import { createServerApi, type ServerApi } from "./server-api.js"
import { createWorkspaceApi, type WorkspaceApi } from "./workspace-api.js"
import { createCredentialApi, type CredentialApi } from "./credential-api.js"
import { createGithubApi, type GithubApi } from "./github-api.js"
import { createPairingApi, type PairingApi } from "./pairing-api.js"
import { createAuthApi, type AuthApi } from "./auth-api.js"
import { createCatalogueReadApi, type CatalogueReadApi } from "./catalogue-read-api.js"
import { createRecordReadApi, type RecordReadApi } from "./record-read-api.js"
import {
  createThreadCommands,
  type SendMessageOptions,
  type ThreadMessageReceipt,
  type StopThreadOptions,
  type StopThreadResult,
} from "./thread-api.js"
import {
  openThreadSubscription,
  type ThreadSubscription,
  type ThreadSubscribeOptions,
} from "./thread-subscription.js"
import type {
  ConversationCreateOptions,
  ConversationSubmission,
} from "./conversation-api.js"
import type {
  ConversationCreateResult,
  ConversationMessageReceipt,
} from "../generated/product.js"

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
  private readonly threadCommands: ReturnType<typeof createThreadCommands>
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
  /** Bounded committed physical records for an authorized receiver. */
  readonly records: RecordReadApi
  /** Finite owner catalogue reads under current receiver authority. */
  readonly catalogue: CatalogueReadApi
  /** Replay-to-live conversation views and lists on this connection. */
  readonly subscriptions: SubscriptionApi
  /** Configured agents, models, and provider-specific approval choices. */
  readonly agents: AgentsApi
  /** Stage files into a conversation so a message can refer to them by digest. */
  readonly attachments: AttachmentApi
  /** An MCP App's calls to its own server, its resources, and its release. */
  readonly mcpApps: McpAppsApi
  /** The gateway's stored MCP servers: list, save, remove, and inspect one. */
  readonly mcpServers: McpServersApi
  /** Issue, list, and revoke scoped product credentials, subject to server authorization. */
  readonly credentials: CredentialApi
  /** Owner pairing: create a one-use code, and approve, deny or cancel an invitation. */
  readonly pairing: PairingApi
  /** The gateway's GitHub sign-in: read it, start it, cancel it, or forget it. */
  readonly github: GithubApi
  /** Fetch a fresh snapshot of the authenticated product identity and restrictions. */
  readonly auth: AuthApi
  /** Tell the gateway what this window shows, for the person's agents to read. */
  readonly workspace: WorkspaceApi

  private constructor(
    private readonly wire: ManagedSession,
    /** Authenticated connection profile. */
    readonly profile: "product",
    newRequestId: () => string,
    upload: AttachmentUploadTransport,
    resources: McpResourceTransport,
    httpTimer: RequestTimer,
  ) {
    this.server = createServerApi(wire)
    this.conversation = createConversationApi(wire, newRequestId)
    this.threadCommands = createThreadCommands(this.conversation, newRequestId, wire)
    this.records = createRecordReadApi(wire)
    this.catalogue = createCatalogueReadApi(wire)
    this.subscriptions = createSubscriptionApi(wire)
    this.agents = createAgentsApi(wire)
    this.attachments = createAttachmentApi(wire, upload, newRequestId, httpTimer)
    this.mcpApps = createMcpAppsApi(wire, resources, newRequestId, httpTimer)
    this.mcpServers = createMcpServersApi(wire)
    this.credentials = createCredentialApi(wire, newRequestId)
    this.pairing = createPairingApi(wire)
    this.github = createGithubApi(wire)
    this.auth = createAuthApi(wire)
    this.workspace = createWorkspaceApi(wire)
  }

  /** Validate options, authenticate, and resolve only when a session is ready.
   * Product setup retries eligible transient failures according to config.retry.
   * Authentication and version rejection stop immediately.
   * @param options - Profile, caller metadata, credentials, and optional connection tuning.
   * @returns A connected client that owns its transport and recovery lifecycle.
   * @throws StageConfigError for invalid options, NessaProtocolCompatibilityError for
   * incompatible versions, or an RPC/transport error when setup fails. */
  static async connect(options: NessaClientConnectOptions): Promise<NessaClient> {
    const { managed, profile, newRequestId, upload, resources, httpTimer } =
      await establishManagedSession(options, NessaClient.defaultUrl)
    return new NessaClient(managed, profile, newRequestId, upload, resources, httpTimer)
  }

  /**
   * Create a thread using the authenticated owner's gateway defaults, or reopen
   * its original identity. The returned conversationId is the thread ID used by
   * sendMessage and subscribe. The gateway answers when its agent is ready.
   * @param options - Optional stable IDs and agent, model, environment and approval choices.
   * @returns The existing conversation creation receipt.
   * @throws NessaConversationMutationError with original IDs and retry() on failure.
   */
  createThread(options?: ConversationCreateOptions): Promise<ConversationCreateResult> {
    return this.threadCommands.createThread(options)
  }

  /**
   * Send text to a thread. Queued input starts a turn when the agent is ready;
   * steering asks the provider to inject it and preserves its actual admission
   * receipt. Passive saves an attributed record-only note without starting,
   * steering, or arranging delivery to the model.
   * @param threadId - The conversationId returned by createThread.
   * @param text - Nonblank text, at most 8 KiB UTF-8, kept exactly.
   * @param options - Queued by default; reuse requestId with identical inputs for a retry.
   * @returns A turn admission receipt, or a passive note receipt with deliveryState.
   * Neither result promises model consumption or completion.
   * @throws TypeError for invalid input; NessaConversationMutationError retains
   * original immutable inputs for explicit retry(). Reconnection does not replay it.
   */
  sendMessage(
    threadId: string,
    text: string,
    options: SendMessageOptions & { mode: "passive" },
  ): Promise<ConversationMessageReceipt>
  sendMessage(
    threadId: string,
    text: string,
    options?: SendMessageOptions & { mode?: "queued" | "steering" },
  ): Promise<ConversationSubmission>
  sendMessage(
    threadId: string,
    text: string,
    options?: SendMessageOptions,
  ): Promise<ThreadMessageReceipt>
  sendMessage(
    threadId: string,
    text: string,
    options?: SendMessageOptions,
  ): Promise<ThreadMessageReceipt> {
    return this.threadCommands.sendMessage(threadId, text, options)
  }

  /**
   * Follow committed semantic events without opening an agent. Consume each batch
   * in order and save its cursor after applying it. A slow consumer ends with a
   * typed lag error instead of silently dropping events. Connection loss ends the
   * stream; subscribe again from your last applied cursor after recovery.
   * @param threadId - The conversationId returned by createThread.
   * @param options - Resume cursor, optional event kinds, and abort signal.
   * @returns A single-consumer async iterable. for-await break or close releases it.
   * @throws TypeError for invalid input, RPC errors when opening fails, or
   * NessaThreadSubscriptionError during iteration when the stream ends.
   */
  subscribe(
    threadId: string,
    options?: ThreadSubscribeOptions,
  ): Promise<ThreadSubscription> {
    return openThreadSubscription(this.subscriptions, threadId, options)
  }

  /**
   * Stop one captured turn while keeping the thread and its agent attachment.
   * Without executionId, one passive snapshot selects the running turn; if none
   * runs, outcome is no_active_turn and queued work is unchanged. A turn that
   * starts after capture is not substituted for the selected turn.
   * @param threadId - The conversationId returned by createThread.
   * @param options - Optional exact execution and action attribution.
   * @returns Existing stop evidence, or the explicit no_active_turn result.
   * @throws TypeError for invalid input; NessaConversationControlError preserves
   * uncertainty. Read current state before deliberately trying another stop.
   */
  stopThread(threadId: string, options?: StopThreadOptions): Promise<StopThreadResult> {
    return this.threadCommands.stopThread(threadId, options)
  }

  /** Current authenticated handshake snapshot; available only while connected. */
  get productSession(): ProductSessionReady {
    const ready = this.wire.ready
    if (!ready) throw new Error("session is not connected")
    return ready
  }

  /** Subscribe to a typed protocol event. Returns an unsubscribe function. A registration still held survives transient recovery; a second subscription of the same function shares it, and either unsubscribe removes it, so recovery does not bring a removed registration back. Missed events are not replayed, except `permission.reviewWaiting`, which carries the whole waiting set: a new subscription is handed the latest one this connection received. */
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
