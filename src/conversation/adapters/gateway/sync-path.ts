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
 *
 * The managed session reconnects this socket on its own. Watch ids do not
 * survive that: `reconnecting` suspends the follow, and `connected`
 * registers the catalogue and the record watch again. A record the server
 * no longer has (`wrong_owner`, `wrong_receiver`, `stale_epoch`) drops only
 * that watch.
 */
import { NessaRpcError } from "@nessa/client"

/** Why the follow stopped and the window's timers should resume. */
export type FollowFallback = "watch-refused" | "watch-ended"

/** What `start` decided. The window backs off on `retry` and stops on `refused`. */
export type FollowStart = "sync" | "refused" | "retry"

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
  start(): Promise<FollowStart>
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
  /**
   * The catalogue watch is held, or the socket is reconnecting and the
   * watches are gone until `connected` registers them again.
   */
  onWatching(held: boolean): void
  onFallback(reason: FollowFallback): void
  /**
   * Waits between capacity retries. Tests pass a stand-in; the desktop
   * waits on the real clock. The gaps are 50, 200, and 500 milliseconds.
   */
  wait?(ms: number): Promise<void>
}

/** A record watch with one of these ends. The catalogue watch on the socket stays. */
const RECORD_DROP = new Set(["wrong_owner", "wrong_receiver", "stale_epoch"])
const RETRY = new Set(["watch_capacity", "watch_duplicate"])
/** Between the four registration attempts. The last gap is also the deferred record retry. */
const RETRY_WAITS_MS = [50, 200, 500] as const

type Registered =
  { watchId: string } | { status: "refused"; code: string } | { status: "retry" }

function codeOf(error: unknown): string | undefined {
  return error instanceof NessaRpcError ? error.code : undefined
}

function watchIdOf(registered: Registered): string | undefined {
  return "watchId" in registered ? registered.watchId : undefined
}

function defaultWait(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms)
  })
}

/**
 * Follows commit pings and tells the window which owner to run.
 *
 * A catalogue ping is `onListChanged`. A record ping is
 * `onConversationChanged` for the conversation that watch names. The
 * catalogue watch ending, the socket closing, or an access refusal on the
 * catalogue is `onFallback`, and the window's timers resume. A record watch
 * the server no longer has drops only that watch.
 */
export function createChangeFollower(options: ChangeFollowOptions): ChangeFollower {
  let stopped = false
  let announced = false
  let suspended = false
  let socket: FollowSocket | undefined
  let catalogueId: string | undefined
  let recordId: string | undefined
  let recordConversation: string | undefined
  /** A record the server refused. Not watched again until the target changes. */
  let skipped: string | undefined
  let generation = 0
  let end: FollowStart = "retry"
  let chain: Promise<void> = Promise.resolve()
  const unlistens: Array<() => void> = []
  const wait = options.wait ?? defaultWait

  const fail = (reason: FollowFallback) => {
    if (stopped) return
    end = reason === "watch-refused" ? "refused" : "retry"
    if (announced) options.onFallback(reason)
    stop()
  }

  async function register(
    attempt: () => Promise<{ watchId: string }>,
    gen: number,
  ): Promise<Registered> {
    for (let tried = 0; tried < RETRY_WAITS_MS.length + 1; tried++) {
      if (stopped || gen !== generation) return { status: "retry" }
      try {
        return await attempt()
      } catch (error) {
        const code = codeOf(error) ?? "unavailable"
        if (RETRY.has(code) && tried < RETRY_WAITS_MS.length) {
          await wait(RETRY_WAITS_MS[tried] ?? 500)
          continue
        }
        if (RETRY.has(code)) return { status: "retry" }
        return { status: "refused", code }
      }
    }
    return { status: "retry" }
  }

  function rememberSkip(target: string | undefined, registered: Registered) {
    if (
      target !== undefined &&
      "status" in registered &&
      registered.status === "refused"
    ) {
      if (RECORD_DROP.has(registered.code)) skipped = target
    }
  }

  async function holdRecord(gen: number): Promise<void> {
    const current = socket
    if (stopped || gen !== generation || current === undefined || suspended) return
    const target = options.recordTargets()[0]
    if (target !== undefined && target !== skipped) skipped = undefined
    if (target === recordConversation && recordId !== undefined && target !== skipped)
      return
    const previous = recordId
    recordId = undefined
    recordConversation = undefined
    if (previous !== undefined) {
      await current.watches.unwatch(previous).catch(() => undefined)
    }
    if (stopped || gen !== generation || suspended) return
    if (target === undefined || target === skipped) {
      options.onRecordHeld(undefined)
      return
    }
    const outcome = await register(
      () => current.watches.records({ conversationId: target }),
      gen,
    )
    if (stopped || gen !== generation || suspended) return
    const accepted = watchIdOf(outcome)
    if (accepted === undefined) {
      if (
        "status" in outcome &&
        outcome.status === "refused" &&
        RECORD_DROP.has(outcome.code)
      ) {
        rememberSkip(target, outcome)
        options.onRecordHeld(undefined)
        return
      }
      if ("status" in outcome && outcome.status === "retry") {
        options.onRecordHeld(undefined)
        const again = gen
        void wait(RETRY_WAITS_MS[RETRY_WAITS_MS.length - 1] ?? 500).then(() => {
          if (stopped || again !== generation || suspended) return
          if (options.recordTargets()[0] !== target) return
          retarget()
        })
        return
      }
      fail("watch-refused")
      return
    }
    recordId = accepted
    recordConversation = target
    options.onRecordHeld(target)
  }

  async function resume(gen: number): Promise<void> {
    const current = socket
    if (stopped || gen !== generation || current === undefined) return
    const catalogue = await register(() => current.watches.catalogue(), gen)
    if (stopped || gen !== generation) return
    const accepted = watchIdOf(catalogue)
    if (accepted === undefined) {
      fail(
        "status" in catalogue && catalogue.status === "retry"
          ? "watch-ended"
          : "watch-refused",
      )
      return
    }
    catalogueId = accepted
    suspended = false
    options.onWatching(true)
    await holdRecord(gen)
  }

  function retarget(): void {
    const gen = generation
    chain = chain.then(() => holdRecord(gen))
  }

  function listen(opened: FollowSocket) {
    unlistens.push(
      opened.on("conversation.changed", (payload) => {
        if (suspended) return
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
    if (!opened.onConnectionStateChange) return
    unlistens.push(
      opened.onConnectionStateChange((state) => {
        if (state.status === "closed") {
          fail("watch-ended")
          return
        }
        if (state.status === "reconnecting") {
          if (suspended || stopped) return
          suspended = true
          generation += 1
          catalogueId = undefined
          recordId = undefined
          recordConversation = undefined
          skipped = undefined
          options.onRecordHeld(undefined)
          options.onWatching(false)
          return
        }
        if (state.status === "connected" && suspended) {
          const gen = generation
          chain = chain.then(() => resume(gen))
        }
      }),
    )
  }

  async function start(): Promise<FollowStart> {
    let opened: FollowSocket
    try {
      opened = await options.openConnection()
    } catch {
      return "retry"
    }
    if (stopped) {
      opened.close()
      return "retry"
    }
    socket = opened
    listen(opened)
    const gen = generation
    const catalogue = await register(() => opened.watches.catalogue(), gen)
    if (stopped) return end
    const accepted = watchIdOf(catalogue)
    if (accepted === undefined) {
      end = "status" in catalogue && catalogue.status === "retry" ? "retry" : "refused"
      stop()
      return end
    }
    catalogueId = accepted
    await holdRecord(gen)
    if (stopped) return end
    announced = true
    return "sync"
  }

  function stop(): void {
    if (stopped && socket === undefined) return
    stopped = true
    suspended = false
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
