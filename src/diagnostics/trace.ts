/**
 * A record of what the panel did, for the times a screenshot is not enough.
 *
 * A user reports "there are twenty of these and I only sent three messages".
 * The gateway's journals say what it received; nothing says what this side did
 * between the click and the paint, so the answer has to be guessed from source.
 * This is that missing half: every action the store took, in order, with the
 * state it produced, written where it can be read back.
 *
 * Off unless asked for. It is turned on by `VITE_NESSA_TRACE` at build time or
 * `localStorage.nessaTrace` at run time, so a build carrying it costs a
 * disabled branch per dispatch and nothing else. Nothing here is a control
 * production behaviour reads: the store does not consult the trace, and a
 * recording that fails is dropped rather than raised.
 *
 *   dispatch ──▶ record(action) ──▶ ring buffer ──▶ console line
 *                                        │
 *                                        └──▶ window.nessaTrace() for a dump
 *
 * The buffer is bounded: a panel left open for a day must not grow without
 * limit, and the interesting part of a repro is always its tail.
 */
import type { Middleware } from "@reduxjs/toolkit"

/** What one dispatch did, small enough to print and to keep many of. */
export type TracedAction = {
  /** Position in this session's recording, so a gap is visible. */
  sequence: number
  /** Milliseconds since the recording began, not a wall clock. */
  at: number
  type: string
  /** How many conversations the store held afterwards, and what each queued. */
  after: { conversations: number; pending: Record<string, number> }
}

const LIMIT = 500
const PREFIX = "[nessa-trace]"

let sequence = 0
let began = 0
const recorded: TracedAction[] = []

/**
 * Whether this panel is recording.
 *
 * Both sources are read every call rather than cached: a recording is turned on
 * to catch something already happening, and asking a person to restart the app
 * first is how the thing being chased disappears.
 */
export function tracing(source: { VITE_NESSA_TRACE?: string } = {}): boolean {
  if (source.VITE_NESSA_TRACE === "1") return true
  try {
    return globalThis.localStorage?.getItem("nessaTrace") === "1"
  } catch {
    // A page with storage blocked is not a page that cannot be debugged; it
    // just cannot be asked at run time.
    return false
  }
}

/** Everything recorded so far, oldest first. */
export function recording(): readonly TracedAction[] {
  return recorded
}

/** Forget what has been recorded, so one repro does not include the last. */
export function clearRecording(): void {
  recorded.length = 0
  sequence = 0
  began = 0
}

type StateShape = {
  conversation?: {
    conversations?: readonly { id: string; remote?: { pending?: readonly unknown[] } }[]
  }
}

/** The part of the state a queue question is asked about, and nothing else. */
function summarize(state: unknown): TracedAction["after"] {
  const conversations = (state as StateShape)?.conversation?.conversations ?? []
  const pending: Record<string, number> = {}
  for (const conversation of conversations) {
    pending[conversation.id] = conversation.remote?.pending?.length ?? 0
  }
  return { conversations: conversations.length, pending }
}

/**
 * Middleware that writes the recording, when one was asked for.
 *
 * Placed after the store's own middleware so what it records is what the
 * reducers actually saw, thunks included.
 */
export function traceMiddleware(enabled: boolean): Middleware {
  return (api) => (next) => (action) => {
    const result = next(action)
    if (!enabled) return result
    try {
      if (began === 0) began = Date.now()
      const entry: TracedAction = {
        sequence: sequence++,
        at: Date.now() - began,
        type:
          typeof action === "object" && action && "type" in action
            ? String((action as { type: unknown }).type)
            : "unknown",
        after: summarize(api.getState()),
      }
      recorded.push(entry)
      if (recorded.length > LIMIT) recorded.shift()
      // One line per action, so a `tauri dev` terminal is a live trace and
      // nobody has to open devtools to read it.
      console.log(`${PREFIX} ${JSON.stringify(entry)}`)
    } catch {
      // A recording that throws would turn a diagnostic into the fault being
      // diagnosed. It is dropped.
    }
    return result
  }
}

/**
 * Count what is mounted under one name, and say where each mount came from.
 *
 * A store recording answers "what did this side do"; it cannot answer "why are
 * there twenty of this component when the state says one". That question is
 * about the tree, and the only witness to it is the stack at mount time. This
 * keeps a live count per name and prints it with that stack, so two mounts of
 * something that should exist once are visible the moment the second appears.
 *
 * Not a hook: it is called from one, but it has no React in it, so the count
 * survives whatever React does to the component around it.
 */
const mounted = new Map<string, number>()

export function mountTraced(name: string, detail: string, enabled: boolean): () => void {
  if (!enabled) return () => {}
  const live = (mounted.get(name) ?? 0) + 1
  mounted.set(name, live)
  // `console.trace` so the parent chain comes with it. A second live mount of
  // something the source mounts once is the finding, so it is said plainly
  // rather than left to be counted by eye.
  const note = live > 1 ? `LIVE=${live} (expected 1)` : `LIVE=${live}`
  console.trace(`${PREFIX} mount ${name} ${note} ${detail}`)
  return () => {
    mounted.set(name, Math.max(0, (mounted.get(name) ?? 1) - 1))
    console.log(`${PREFIX} unmount ${name} LIVE=${mounted.get(name)} ${detail}`)
  }
}

/** How many of each traced name are mounted right now. */
export function liveMounts(): Readonly<Record<string, number>> {
  return Object.fromEntries(mounted)
}

/**
 * Hand the recording to whoever is looking, from a console or a hook.
 *
 * Exposed on `globalThis` rather than exported alone because the person who
 * needs it is usually in a devtools console with no module graph to import
 * from.
 */
export function publishRecording(): void {
  Object.defineProperty(globalThis, "nessaTrace", {
    configurable: true,
    value: () => JSON.stringify(recording(), null, 2),
  })
}
