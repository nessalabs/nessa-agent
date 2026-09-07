import type { ClientEventMap } from "../protocol/index.js"

/** Event-name to payload mapping consumed by NessaClient.on. These are protocol events, separate from connection-state notifications. */
export type NessaClientEvents = ClientEventMap

/** Callback whose payload type is selected by the subscribed event name. */
export type EventHandler<K extends keyof NessaClientEvents> = (
  payload: NessaClientEvents[K],
) => void
