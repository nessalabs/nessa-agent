import type { ClientEventMap } from "../protocol/index.js"

export type NessaClientEvents = ClientEventMap

export type EventHandler<K extends keyof NessaClientEvents> = (
  payload: NessaClientEvents[K],
) => void
