/**
 * Composition root — dependency wiring for a connected client.
 *
 * Like the server's `composition::CompositionRoot`: load transport,
 * run application handshake, return the presentation facade.
 */
export { connectClient } from "./root.js"
