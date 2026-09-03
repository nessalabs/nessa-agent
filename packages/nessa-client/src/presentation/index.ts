/**
 * Presentation layer — stable public API for product code.
 *
 * Surfaces and plugins import `NessaClient` from here (via package root).
 * No wire types or WebSocket details leak into UI code.
 *
 * ```
 * NessaClient.connect(options)
 *      │
 *      ├── client.session   (HelloOk)
 *      ├── client.server.health()
 *      └── client.on("connect.challenge", …)
 * ```
 */
export { NessaClient } from "./nessa-client.js"
export type { NessaClientConnectOptions } from "../application/options.js"
export type { ServerApi } from "./server-api.js"
