/**
 * Both desktop windows follow committed changes with a payloadless ping.
 *
 * The follower only signals "the list changed" or "chat X changed". The
 * window's existing list and read owners do the work: sequenced reads, the
 * taken-out check, timeouts, and the server's list order. This module does
 * not read a view, a head, or a catalogue payload.
 *
 * Watches sit on a connection of their own, never the one commands use. A
 * watch refusal closes that connection, and that close must not sign the
 * desktop out. One connection may watch one conversation and one catalogue,
 * so this window holds the catalogue watch and at most one record watch on
 * that socket. A chat without that record watch keeps its own poll. The
 * catalogue watch replaces only the list timer.
 */
import { NessaRpcError } from "@nessa/client"

/** Why the follow stopped and the window's timers should resume. */
export type FollowFallback = "watch-refused" | "watch-ended"

/** The dedicated watch connection. It is not the command client. */
export interface FollowSocket {
  watches: {
    records(params: { conversationId: string }): Promise<{ watchId: string }>
    catalogue(): Promise<{ watchId: string }>
    unwatch(watchId: string): Promise<unknown>
  }
  on(
    event: "conversation.changed" | "conversation.watchEnded",
    handler: (payload: { watchId: string; reason?: string }) => void,
  ): () => void
  onConnectionStateChange?(handler: (state: { status: string }) => void): () => void
  close(): void
}

export interface ChangeFollower {
  start(): Promise<"sync" | "fallback">
  stop(): void
  /** The record target changed. Overlapping calls run one at a time. */
  retarget(): void
  /** The conversation holding the one record watch, when one is held. */
  recordTarget(): string | undefined
}

export interface ChangeFollowOptions {
  /** A new connection used only for watches. */
  openConnection: () => Promise<FollowSocket>
  /**
   * Highest priority first. The follower holds at most the first id: one
   * connection may watch one conversation.
   */
  recordTargets(): readonly string[]
  onListChanged(): void
  onConversationChanged(conversationId: string): void
  /** The record watch was installed or dropped. A new id is one read's worth. */
  onRecordHeld(conversationId: string | undefined): void
  onFallback(reason: FollowFallback): void
}

const ACCESS = new Set([
  "unauthorized",
  "forbidden",
  "wrong_owner",
  "wrong_receiver",
  "stale_epoch",
])
const RETRY = new Set(["watch_capacity", "watch_duplicate"])

function codeOf(error: unknown): string | undefined {
  return error instanceof NessaRpcError ? error.code : undefined
}

/**
 * Follows commit pings and tells the window which owner to run.
 *
 * A catalogue ping is `onListChanged`. A record ping is
 * `onConversationChanged` for the conversation that watch names. The
 * catalogue watch ending, the socket closing, or an access refusal on
 * register is `onFallback`, and the window's timers resume. A record watch
 * ending drops only that watch.
 */
export function createChangeFollower(options: ChangeFollowOptions): ChangeFollower {
  let stopped = false
  let socket: FollowSocket | undefined
  let catalogueId: string | undefined
  let recordId: string | undefined
  let recordConversation: string | undefined
  let generation = 0
  let chain: Promise<void> = Promise.resolve()
  const unlistens: Array<() => void> = []

  const fail = (reason: FollowFallback) => {
    if (stopped) return
    options.onFallback(reason)
    stop()
  }

  async function register(
    attempt: () => Promise<{ watchId: string }>,
  ): Promise<{ watchId: string } | "refused" | "retry"> {
    for (let tried = 0; tried < 3; tried++) {
      try {
        return await attempt()
      } catch (error) {
        const code = codeOf(error)
        if (code !== undefined && RETRY.has(code) && tried < 2) continue
        if (code !== undefined && RETRY.has(code)) return "retry"
        if (code !== undefined && ACCESS.has(code)) return "refused"
        return "refused"
      }
    }
    return "retry"
  }

  async function holdRecord(gen: number): Promise<void> {
    const current = socket
    if (stopped || gen !== generation || current === undefined) return
    const target = options.recordTargets()[0]
    if (target === recordConversation && recordId !== undefined) return
    const previous = recordId
    recordId = undefined
    recordConversation = undefined
    if (previous !== undefined) {
      await current.watches.unwatch(previous).catch(() => undefined)
    }
    if (stopped || gen !== generation) return
    if (target === undefined) {
      options.onRecordHeld(undefined)
      return
    }
    const outcome = await register(() =>
      current.watches.records({ conversationId: target }),
    )
    if (stopped || gen !== generation) return
    if (outcome === "refused") {
      fail("watch-refused")
      return
    }
    if (outcome === "retry") {
      options.onRecordHeld(undefined)
      return
    }
    recordId = outcome.watchId
    recordConversation = target
    options.onRecordHeld(target)
  }

  function retarget(): void {
    const gen = generation
    chain = chain.then(() => holdRecord(gen))
  }

  async function start(): Promise<"sync" | "fallback"> {
    let opened: FollowSocket
    try {
      opened = await options.openConnection()
    } catch {
      options.onFallback("watch-refused")
      return "fallback"
    }
    if (stopped) {
      opened.close()
      return "fallback"
    }
    socket = opened
    unlistens.push(
      opened.on("conversation.changed", (payload) => {
        if (payload.watchId === catalogueId) options.onListChanged()
        else if (payload.watchId === recordId && recordConversation !== undefined)
          options.onConversationChanged(recordConversation)
      }),
      opened.on("conversation.watchEnded", (payload) => {
        if (payload.watchId === catalogueId) fail("watch-ended")
        else if (payload.watchId === recordId) {
          recordId = undefined
          recordConversation = undefined
          options.onRecordHeld(undefined)
        }
      }),
    )
    if (opened.onConnectionStateChange) {
      unlistens.push(
        opened.onConnectionStateChange((state) => {
          if (state.status === "closed") fail("watch-ended")
        }),
      )
    }
    const catalogue = await register(() => opened.watches.catalogue())
    if (stopped) return "fallback"
    if (catalogue === "refused" || catalogue === "retry") {
      fail("watch-refused")
      return "fallback"
    }
    catalogueId = catalogue.watchId
    await holdRecord(generation)
    if (stopped) return "fallback"
    return "sync"
  }

  function stop(): void {
    if (stopped && socket === undefined) return
    stopped = true
    generation += 1
    for (const unlisten of unlistens) unlisten()
    unlistens.length = 0
    const current = socket
    const ids = [catalogueId, recordId].filter((id): id is string => id !== undefined)
    catalogueId = undefined
    recordId = undefined
    recordConversation = undefined
    socket = undefined
    if (current === undefined) return
    void Promise.all(
      ids.map((id) => current.watches.unwatch(id).catch(() => undefined)),
    ).finally(() => current.close())
  }

  return {
    start,
    stop,
    retarget,
    recordTarget: () => recordConversation,
  }
}
