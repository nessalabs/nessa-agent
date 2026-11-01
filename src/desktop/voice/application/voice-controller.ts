/**
 * The window's voice call: the one owner of whether a call is open, what
 * was heard and not yet sent, and when the voice may speak
 * (`docs/design/voice-chat.md`, rows V1–V17).
 *
 * The voice is a coordinator. It decides when the person asked the agent for
 * something and calls `send_to_agent`; this sends the person's own
 * transcribed words, tidied, as the composer would. When the agent's turn
 * ends, its reply is handed to the voice to talk about. The voice never
 * writes the message itself, so the agent reads what was said.
 */
import { VoiceToolName } from "@nessa/client"
import { tidy } from "../model/tidy"
import {
  TransportRefused,
  VoiceStartRefused,
  type AgentReply,
  type RealtimeConnection,
  type RealtimeEvent,
  type RealtimeTransports,
  type VoiceConversation,
  type VoiceConversations,
  type VoiceGateway,
  type VoiceTimers,
} from "./ports"

/** How long `send_to_agent` waits for a transcript it covers (V9). */
export const TRANSCRIPT_WAIT_MS = 5_000
/** The most of a reply the voice is handed; the rest is on screen. */
export const MAX_REPLY_CHARS = 8_000

/**
 * Where the call stands. `working`: the agent is on a request the voice
 * sent. `speaking`: the voice is answering. `failed`: no call started.
 * `dropped`: an open call ended on its own.
 */
export type VoicePhase =
  "idle" | "connecting" | "listening" | "working" | "speaking" | "failed" | "dropped"

export interface VoiceSnapshot {
  readonly phase: VoicePhase
  /** The conversation the call talks to, or failed or dropped on; null when idle. */
  readonly sessionId: string | null
  /** The person's words as they are heard, until their turn is written down. */
  readonly hearing: string
  /** What the voice is saying now. */
  readonly saying: string
  readonly microphoneOn: boolean
  readonly speakerOn: boolean
  /** Why the call failed or dropped, for the person. */
  readonly notice: string | null
}

/** What `send_to_agent` and `stop_agent` answer the voice. */
export type ToolOutcome =
  | "sent"
  | "not_sent"
  /** The gateway did not answer: the message may or may not have gone. */
  | "unknown"
  | "nothing_heard"
  | "stopped"
  | "nothing_running"
  | "unknown_tool"

const idle: VoiceSnapshot = {
  phase: "idle",
  sessionId: null,
  hearing: "",
  saying: "",
  microphoneOn: true,
  speakerOn: true,
  notice: null,
}

const silent = { input: 0, output: 0 } as const

/** One user item the voice's model heard: its transcript once written down. */
interface Heard {
  text: string | null
  sent: boolean
}

/** A call from start to end. A newer call, or an end, makes it stale. */
interface Call {
  readonly generation: number
  readonly sessionId: string
  readonly conversation: VoiceConversation
  connection: RealtimeConnection | null
  stopWatching: (() => void) | null
  /** User items in the order the provider committed them. */
  readonly order: string[]
  readonly heard: Map<string, Heard>
  /** Waiting `send_to_agent`s, told when a transcript arrives. */
  readonly waiters: Set<() => void>
  readonly timers: Set<unknown>
  /** Aborted when the call ends, so a connect still under way lets go. */
  readonly abort: AbortController
  responding: boolean
  /** The `event_id` of the `response.create` not yet answered or refused. */
  awaitingCreated: string | null
  responseQueued: boolean
  /** Sends on their way to the agent, not answered yet (shown as working). */
  sending: number
  /**
   * `send_to_agent`s admitted and not yet answered, including those still
   * waiting for a transcript or for an earlier send. No reply is handed
   * over while any is (V12).
   */
  pending: number
  /**
   * The last `send_to_agent` admitted, settled once it has sent or given
   * up. Each send waits for the one before it, transcript wait included,
   * so a later request never reaches the agent ahead of an earlier one.
   */
  sends: Promise<void>
  /**
   * The turns the voice started, by their names, whose answer it has not
   * handed over. Whether each has ended is the conversation's to say; once
   * every one has, what they said is handed over together (V11, V12). A
   * turn typed in the composer is never among them, so it is not read out.
   */
  readonly waiting: string[]
  /** Each waiting turn's end, kept once seen: the view may later leave it out. */
  readonly ends: Map<string, AgentReply>
  /** Waiting turns the conversation has shown, so one gone from it since has ended. */
  readonly seen: Set<string>
  /** How many responses this call asked for, to name each. */
  responses: number
}

export interface VoiceControllerDependencies {
  readonly gateway: VoiceGateway
  readonly transports: RealtimeTransports
  readonly conversations: VoiceConversations
  readonly timers: VoiceTimers
}

export class VoiceController {
  private state: VoiceSnapshot = idle
  private call: Call | null = null
  private generation = 0
  private readonly listeners = new Set<() => void>()
  /** How many panes show each conversation's call. */
  private readonly shown = new Map<string, number>()

  constructor(private readonly dependencies: VoiceControllerDependencies) {}

  snapshot(): VoiceSnapshot {
    return this.state
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  levels(): { readonly input: number; readonly output: number } {
    return this.call?.connection?.levels() ?? silent
  }

  /** V1: start a call about `sessionId`, ending any other first. */
  async start(sessionId: string): Promise<void> {
    if (this.call) this.end()
    const generation = ++this.generation
    const call: Call = {
      generation,
      sessionId,
      conversation: this.dependencies.conversations(sessionId),
      connection: null,
      stopWatching: null,
      order: [],
      heard: new Map(),
      waiters: new Set(),
      timers: new Set(),
      abort: new AbortController(),
      responding: false,
      awaitingCreated: null,
      responseQueued: false,
      sending: 0,
      pending: 0,
      sends: Promise.resolve(),
      waiting: [],
      ends: new Map(),
      seen: new Set(),
      responses: 0,
    }
    this.call = call
    this.set({ ...idle, phase: "connecting", sessionId })
    let grant
    try {
      grant = await this.dependencies.gateway.startCall(sessionId)
    } catch (error) {
      // V2
      if (this.current(call))
        this.fail(
          error instanceof VoiceStartRefused
            ? error.sentence
            : "Couldn’t start the call.",
        )
      return
    }
    if (!this.current(call)) return // V5
    let connection: RealtimeConnection
    try {
      connection = await this.dependencies.transports[grant.transport].connect(
        grant,
        call.abort.signal,
      )
    } catch (error) {
      // V3
      if (this.current(call))
        this.fail(
          error instanceof TransportRefused
            ? error.sentence
            : "Couldn’t connect the call.",
        )
      return
    }
    if (!this.current(call)) {
      connection.close() // V5: ended while it connected
      return
    }
    // V4
    call.connection = connection
    connection.onEvent((event) => {
      if (this.current(call)) this.heardEvent(call, event)
    })
    connection.onClosed(() => {
      // V17
      if (!this.current(call)) return
      this.release(call)
      this.call = null
      this.set({
        ...idle,
        phase: "dropped",
        notice: "The call dropped.",
        sessionId: call.sessionId,
      })
    })
    connection.setMicrophone(this.state.microphoneOn)
    connection.setSpeaker(this.state.speakerOn)
    const context = call.conversation.recentTurns().trim()
    if (context) this.tell(call, `Chat so far:\n${context}`)
    call.stopWatching = call.conversation.onChange(() => {
      if (this.current(call)) this.replied(call)
    })
    this.update(call)
  }

  /** V16: end the call, whatever it is doing. */
  end(): void {
    const call = this.call
    this.generation++
    this.call = null
    if (call) this.release(call)
    this.set(idle)
  }

  /**
   * A pane shows `sessionId`'s call while the answer is held. Once no pane
   * shows the conversation a call talks to, the call ends (V16): a call is
   * never left talking to a chat nobody can see.
   */
  show(sessionId: string): () => void {
    this.shown.set(sessionId, (this.shown.get(sessionId) ?? 0) + 1)
    let held = true
    return () => {
      if (!held) return
      held = false
      const left = (this.shown.get(sessionId) ?? 1) - 1
      if (left > 0) {
        this.shown.set(sessionId, left)
        return
      }
      this.shown.delete(sessionId)
      if (this.state.sessionId === sessionId) this.end()
    }
  }

  /** Puts a failed or dropped call's notice away. */
  dismiss(): void {
    if (this.state.phase === "failed" || this.state.phase === "dropped") this.set(idle)
  }

  /** V15 */
  setMicrophone(on: boolean): void {
    const connection = this.call?.connection
    connection?.setMicrophone(on)
    // Half-heard speech from before the mute is not sent later.
    if (!on) connection?.send({ type: "input_audio_buffer.clear" })
    this.set({ ...this.state, microphoneOn: on })
  }

  /** V15 */
  setSpeaker(on: boolean): void {
    this.call?.connection?.setSpeaker(on)
    this.set({ ...this.state, speakerOn: on })
  }

  private current(call: Call): boolean {
    return this.call === call && call.generation === this.generation
  }

  private release(call: Call): void {
    call.abort.abort()
    call.stopWatching?.()
    call.stopWatching = null
    for (const handle of call.timers) this.dependencies.timers.clearTimeout(handle)
    call.timers.clear()
    for (const wake of call.waiters) wake()
    call.waiters.clear()
    call.connection?.close()
    call.connection = null
  }

  private fail(notice: string): void {
    const call = this.call
    if (call) this.release(call)
    this.call = null
    this.set({ ...idle, phase: "failed", notice, sessionId: call?.sessionId ?? null })
  }

  private set(next: VoiceSnapshot): void {
    this.state = next
    for (const listener of [...this.listeners]) listener()
  }

  /** The phase an open call is in, from what it is doing. */
  private update(call: Call, change: Partial<VoiceSnapshot> = {}): void {
    if (!this.current(call)) return
    const phase = call.responding
      ? "speaking"
      : call.sending > 0 || call.waiting.length > 0
        ? "working"
        : "listening"
    this.set({ ...this.state, ...change, phase, sessionId: call.sessionId })
  }

  private heardEvent(call: Call, event: RealtimeEvent): void {
    const item = typeof event.item_id === "string" ? event.item_id : null
    switch (event.type) {
      case "input_audio_buffer.speech_started":
        this.update(call, { hearing: "" })
        return
      case "input_audio_buffer.committed":
        if (item) this.committed(call, item)
        return
      case "conversation.item.input_audio_transcription.delta":
        if (typeof event.delta === "string")
          this.update(call, { hearing: this.state.hearing + event.delta })
        return
      case "conversation.item.input_audio_transcription.completed":
      case "conversation.item.input_audio_transcription.failed":
        // A transcript that failed is written down as nothing heard, so a
        // send waiting on it goes on at once (V9).
        if (!item) return
        this.committed(call, item).text =
          typeof event.transcript === "string" ? event.transcript : ""
        for (const wake of [...call.waiters]) wake()
        this.update(call, { hearing: "" })
        return
      case "response.created":
        // Which request a response answers is not said: the provider starts
        // its own after the person speaks. Ours stays awaited until a
        // response is done; a refusal of it comes before that.
        call.responding = true
        this.update(call, { saying: "" })
        return
      case "response.output_audio_transcript.delta":
        if (typeof event.delta === "string")
          this.update(call, { saying: this.state.saying + event.delta })
        return
      case "response.done":
        call.responding = false
        call.awaitingCreated = null
        // A tool is called only by a response that completed: one the
        // person talked over is cancelled, and so is what it asked for.
        for (const tool of completedCalls(event))
          void this.toolCalled(call, tool.name, tool.callId)
        this.next(call)
        return
      case "error":
        this.refused(call, event)
        return
    }
  }

  /** The user item `item`, committed in order the first time it is named. */
  private committed(call: Call, item: string): Heard {
    const known = call.heard.get(item)
    if (known) return known
    const heard: Heard = { text: null, sent: false }
    call.order.push(item)
    call.heard.set(item, heard)
    return heard
  }

  private async toolCalled(call: Call, name: string, callId: string): Promise<void> {
    let outcome: ToolOutcome
    switch (name) {
      case VoiceToolName.SendToAgent:
        outcome = await this.sendHeard(call)
        break
      case VoiceToolName.StopAgent:
        // V13
        outcome = (await call.conversation.stop()) ? "stopped" : "nothing_running"
        break
      default:
        outcome = "unknown_tool"
    }
    if (!this.current(call)) return
    call.connection?.send({
      type: "conversation.item.create",
      item: { type: "function_call_output", call_id: callId, output: outcome },
    })
    this.respond(call)
  }

  /** V8–V10: send what was heard and not yet sent, in the person's words. */
  private async sendHeard(call: Call): Promise<ToolOutcome> {
    // Only what was committed before the call: later speech is a later request.
    const covered = call.order
      .map((item) => this.committed(call, item))
      .filter((heard) => !heard.sent)
    for (const heard of covered) heard.sent = true
    call.pending++
    // Sends go in the order they were asked for (V8).
    const before = call.sends
    let settled = () => {}
    call.sends = new Promise((resolve) => (settled = resolve))
    try {
      await before
      return await this.sendCovered(call, covered)
    } finally {
      settled()
    }
  }

  private async sendCovered(call: Call, covered: readonly Heard[]): Promise<ToolOutcome> {
    const written = () => covered.every((heard) => heard.text !== null)
    if (!written()) await this.waitFor(call, written)
    if (!this.current(call)) return "not_sent"
    const text = covered
      .map((heard) => tidy(heard.text ?? ""))
      .filter((part) => part !== "")
      .join(" ")
    if (text === "") {
      call.pending--
      // A reply held back for this send can go now.
      this.replied(call)
      this.update(call)
      return "nothing_heard"
    }
    call.sending++
    this.update(call)
    const { result, turn } = await call.conversation.send(text)
    if (!this.current(call)) return result
    call.sending--
    call.pending--
    // A message that did not go, or may not have, is not waited on: its
    // answer, if any, is on screen. The voice is told which it was.
    if (result === "sent") call.waiting.push(turn)
    // The turn may have ended already, before the send was answered.
    this.replied(call)
    this.update(call)
    return result
  }

  /** Until `done` holds, the call ends, or `TRANSCRIPT_WAIT_MS` passes. */
  private waitFor(call: Call, done: () => boolean): Promise<void> {
    return new Promise((resolve) => {
      const finish = () => {
        call.waiters.delete(check)
        call.timers.delete(handle)
        this.dependencies.timers.clearTimeout(handle)
        resolve()
      }
      const check = () => {
        if (done() || !this.current(call)) finish()
      }
      const handle = this.dependencies.timers.setTimeout(finish, TRANSCRIPT_WAIT_MS)
      call.timers.add(handle)
      call.waiters.add(check)
    })
  }

  /**
   * V11, V12: once every turn the voice started has ended (and no send is
   * still on its way), what they said is handed to the voice, once.
   */
  private replied(call: Call): void {
    if (call.waiting.length === 0) return
    for (const turn of call.waiting) {
      if (call.ends.has(turn)) continue
      const state = call.conversation.turnState(turn)
      if (state.kind === "ended") call.ends.set(turn, state.reply)
      else if (state.kind === "open") call.seen.add(turn)
      // Shown before and gone now: it ended past what the window holds.
      else if (call.seen.has(turn)) call.ends.set(turn, { text: "", ended: "finished" })
    }
    if (call.pending > 0 || call.waiting.some((turn) => !call.ends.has(turn))) return
    const ends = call.waiting.map((turn) => call.ends.get(turn))
    call.waiting.length = 0
    call.ends.clear()
    call.seen.clear()
    const reply: AgentReply = {
      text: ends
        .map((end) => end?.text.trim() ?? "")
        .filter((text) => text !== "")
        .join("\n\n"),
      ended: ends.at(-1)?.ended ?? "finished",
    }
    const text = reply.text.slice(0, MAX_REPLY_CHARS)
    const said =
      reply.ended === "finished"
        ? `Agent reply:\n${text || "(The agent finished without saying anything.)"}`
        : `Agent reply: the agent's turn ${reply.ended === "stopped" ? "was stopped" : "failed"}.${text ? `\n${text}` : ""}`
    this.tell(call, said)
    this.respond(call)
    this.update(call)
  }

  /** Context the voice reads and does not answer by itself. */
  private tell(call: Call, text: string): void {
    call.connection?.send({
      type: "conversation.item.create",
      item: { type: "message", role: "system", content: [{ type: "input_text", text }] },
    })
  }

  /** V14: one response at a time; asked again once the one speaking is done. */
  private respond(call: Call): void {
    if (call.responding) {
      call.responseQueued = true
      return
    }
    call.responding = true
    const eventId = `voice_response_${++call.responses}`
    call.awaitingCreated = eventId
    call.connection?.send({ type: "response.create", event_id: eventId })
    this.update(call)
  }

  /**
   * The provider refused something. Only a refusal of the `response.create`
   * still waiting changes anything: refused because the provider is already
   * answering (it starts a response itself after the person speaks), it is
   * asked again once that one is done; refused otherwise, the voice is free.
   */
  private refused(call: Call, event: RealtimeEvent): void {
    const error = event.error as { code?: unknown; event_id?: unknown } | undefined
    console.warn("The voice call reported an error", error)
    if (!call.awaitingCreated || error?.event_id !== call.awaitingCreated) return
    call.awaitingCreated = null
    if (error.code === "conversation_already_has_active_response") {
      call.responseQueued = true
      return
    }
    call.responding = false
    this.next(call)
  }

  /** The voice is free: the response it held back, if any, is asked for. */
  private next(call: Call): void {
    if (call.responseQueued) {
      call.responseQueued = false
      this.respond(call)
    }
    this.update(call)
  }
}

/** The function calls a `response.done` carries, when the response completed. */
function completedCalls(event: RealtimeEvent): { name: string; callId: string }[] {
  const response = event.response as { status?: unknown; output?: unknown } | undefined
  if (response?.status !== "completed" || !Array.isArray(response.output)) return []
  return response.output.flatMap((item: unknown) => {
    const call = item as { type?: unknown; name?: unknown; call_id?: unknown }
    return call.type === "function_call" && typeof call.call_id === "string"
      ? [{ name: typeof call.name === "string" ? call.name : "", callId: call.call_id }]
      : []
  })
}
