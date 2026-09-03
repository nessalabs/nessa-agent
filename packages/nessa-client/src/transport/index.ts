/**
 * Transport layer — WebSocket I/O and request/response correlation.
 *
 * Owns the socket lifecycle hooks and pending RPC registry. Does not know
 * connect handshake order or the public `NessaClient` API.
 *
 * ```
 * WireSession.request(method, params)
 *      │
 *      ▼
 * encodeWireMessage ──► WebSocket send
 *      │
 *      ▼
 * parseWireMessage ◄── WebSocket message
 *      │
 *      └── resolve matching pending Promise
 * ```
 */
export { waitForSocketOpen } from "./socket.js"
export { WireSession } from "./wire-session.js"
