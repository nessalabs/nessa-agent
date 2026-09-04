import type { HelloOk } from "../protocol/index.js"
import type { WireSession } from "../transport/wire-session.js"
import type { EventHandler, NessaClientEvents } from "../application/events.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { establishSession } from "../composition/root.js"
import {
  createConversationApi,
  type ConversationApi,
} from "./conversation-api.js"
import { createServerApi, type ServerApi } from "./server-api.js"

/**
 * Public SDK facade — what surfaces, bridges, and tests import.
 *
 * Presentation layer only: delegates connect to composition and RPCs to
 * transport via typed namespaces (`server.health`, `conversation.echo`, …).
 */
export class NessaClient {
  static readonly defaultUrl = "ws://127.0.0.1:7420"

  readonly server: ServerApi
  readonly conversation: ConversationApi

  private constructor(
    private readonly wire: WireSession,
    private hello: HelloOk | null,
  ) {
    this.server = createServerApi(wire)
    this.conversation = createConversationApi(wire)
  }

  static async connect(options: NessaClientConnectOptions): Promise<NessaClient> {
    const { wire, hello } = await establishSession(options, NessaClient.defaultUrl)
    return new NessaClient(wire, hello)
  }

  get session(): HelloOk {
    if (!this.hello) throw new Error("NessaClient.connect() required before use")
    return this.hello
  }

  on<K extends keyof NessaClientEvents>(event: K, handler: EventHandler<K>): () => void {
    return this.wire.onEvent(event, handler as (payload: unknown) => void)
  }

  onClose(handler: () => void): () => void {
    return this.wire.onClose(handler)
  }

  close(): void {
    this.hello = null
    this.wire.close()
  }
}
