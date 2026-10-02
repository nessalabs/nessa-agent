/**
 * Transport layer — WebSocket I/O and request/response correlation.
 *
 * Owns the socket lifecycle hooks, pending RPC registry, and raw response byte
 * ceiling. A larger record response is accepted only for a pending record read.
 * Does not know
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
export {
  LocalGatewayEndpointSource,
  nodeGatewayEndpointSource,
} from "./local-gateway-endpoint.js"
export { WireSession } from "./wire-session.js"
