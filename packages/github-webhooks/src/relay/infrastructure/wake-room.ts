interface RelaySocket extends WebSocket {
  serializeAttachment(value: unknown): void
  deserializeAttachment(): { expiresAt: number }
}
export interface RoomState {
  acceptWebSocket(socket: WebSocket): void
  getWebSockets(): RelaySocket[]
  storage: { getAlarm(): Promise<number | null>; setAlarm(time: number): Promise<void> }
}
declare const WebSocketPair: { new (): { 0: WebSocket; 1: RelaySocket } }
export interface WakeRooms {
  getByName(name: string): { fetch(request: Request): Promise<Response> }
}

/** Only the outer Worker can reach this class. It receives no GitHub credential
 * or event payload. Stored socket attachments are expiry metadata only. */
export class WakeRoom {
  constructor(
    private readonly state: RoomState,
    private readonly now: () => number,
  ) {}
  async fetch(request: Request): Promise<Response> {
    if (request.method === "POST") {
      for (const socket of this.state.getWebSockets()) {
        if (socket.deserializeAttachment().expiresAt <= this.now())
          socket.close(1000, "renew")
        else {
          try {
            socket.send('{"type":"changed"}')
          } catch {
            socket.close(1011, "reconnect")
          }
        }
      }
      return new Response(null, { status: 204 })
    }
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket")
      return new Response(null, { status: 426 })
    if (this.state.getWebSockets().length >= 128)
      return new Response(null, { status: 503 })
    const expiresAt = Number(request.headers.get("x-relay-expires"))
    if (!Number.isSafeInteger(expiresAt) || expiresAt <= this.now())
      return new Response(null, { status: 401 })
    const alarm = await this.state.storage.getAlarm()
    if (alarm === null || alarm > expiresAt) await this.state.storage.setAlarm(expiresAt)
    if (expiresAt <= this.now()) return new Response(null, { status: 401 })
    const pair = new WebSocketPair()
    pair[1].serializeAttachment({ expiresAt })
    this.state.acceptWebSocket(pair[1])
    return new Response(null, { status: 101, webSocket: pair[0] } as ResponseInit)
  }
  async alarm() {
    let next = Infinity
    for (const socket of this.state.getWebSockets()) {
      const expiry = socket.deserializeAttachment().expiresAt
      if (expiry <= this.now()) socket.close(1000, "renew")
      else next = Math.min(next, expiry)
    }
    if (next !== Infinity) await this.state.storage.setAlarm(next)
  }
  webSocketMessage(socket: WebSocket) {
    socket.close(1008, "receive_only")
  }
  webSocketClose(socket: WebSocket) {
    socket.close(1000, "peer_closed")
  }
  webSocketError(socket: WebSocket) {
    socket.close(1011, "reconnect")
  }
}
