import type { HelloOk } from "../protocol/index.js"
import type { WireSession } from "../transport/wire-session.js"
import type { EventHandler, NessaClientEvents } from "../application/events.js"
import type { NessaClientConnectOptions } from "../application/options.js"
import { connectClient } from "../composition/root.js"
import { createServerApi, type ServerApi } from "./server-api.js"

/**
 * Public SDK facade — what surfaces, bridges, and tests import.
 *
 * Presentation layer only: delegates connect to composition and RPCs to
 * transport via typed namespaces (`server.health`, …).
 */
export class NessaClient {
  static readonly defaultUrl = "ws://127.0.0.1:7420"

  readonly server: ServerApi

  private constructor(
    private readonly wire: WireSession,
    private hello: HelloOk | null,
  ) {
    this.server = createServerApi(wire)
  }

  static connect(options: NessaClientConnectOptions): Promise<NessaClient> {
    return connectClient(options)
  }

  /** @internal Used by composition root after handshake. */
  static fromSession(wire: WireSession, hello: HelloOk): NessaClient {
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

  /** @internal Test seam for frame dispatch. */
  dispatchFrame(frame: import("../protocol/index.js").Frame): void {
    this.wire.dispatchFrame(frame)
  }

  close(): void {
    this.hello = null
    this.wire.close()
  }
}
