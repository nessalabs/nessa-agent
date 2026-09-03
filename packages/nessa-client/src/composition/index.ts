/**
 * Composition root — dependency wiring for a connected client.
 *
 * Like the server's `composition::CompositionRoot`: load transport,
 * run application handshake, return the established session for presentation.
 */
export { establishSession, type EstablishedSession } from "./root.js"
