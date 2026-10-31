/**
 * What a voice call needs from outside the window's own code, each injected
 * from composition (`main.tsx` via `createDesktopDependencies`) with a
 * substitute in tests. The call's rules are `VoiceController`'s.
 */
import type { VoiceCall, VoiceTransport } from "@nessa/client"

/** One realtime event, in OpenAI's realtime vocabulary, either way. */
export type RealtimeEvent = { readonly type: string } & Record<string, unknown>

/** An open call: realtime events both ways, and its audio's controls. */
export interface RealtimeConnection {
  send(event: RealtimeEvent): void
  /** Every event the provider sends, in order. Set once, before any arrives. */
  onEvent(listener: (event: RealtimeEvent) => void): void
  /** The call ended from the far side or the network. Not called after `close`. */
  onClosed(listener: () => void): void
  /** How loud the person (`input`) and the voice (`output`) are now, 0–1. */
  levels(): { readonly input: number; readonly output: number }
  setMicrophone(on: boolean): void
  setSpeaker(on: boolean): void
  close(): void
}

/** Why a transport could not open a call, said to the person. */
export class TransportRefused extends Error {
  constructor(readonly sentence: string) {
    super(sentence)
    this.name = "TransportRefused"
  }
}

/**
 * Opens a call the gateway minted. Throws `TransportRefused` when it cannot.
 * Once `signal` aborts — the call ended while it connected — it lets go of
 * everything it took (the microphone above all) and throws.
 */
export interface RealtimeTransport {
  connect(call: VoiceCall, signal: AbortSignal): Promise<RealtimeConnection>
}

/** Every transport the gateway may name, so one it adds is a type error here. */
export type RealtimeTransports = Readonly<Record<VoiceTransport, RealtimeTransport>>

/** Why the gateway started no call, said to the person. */
export class VoiceStartRefused extends Error {
  constructor(readonly sentence: string) {
    super(sentence)
    this.name = "VoiceStartRefused"
  }
}

/** The gateway's side of a call. Throws `VoiceStartRefused` when refused. */
export interface VoiceGateway {
  startCall(conversationId: string): Promise<VoiceCall>
}

/** How an agent's turn ended, as the voice is told. */
export type TurnEnd = "finished" | "stopped" | "failed"

/** What the agent said when a turn ended. */
export interface AgentReply {
  readonly text: string
  readonly ended: TurnEnd
}

/** What became of a message sent: taken, refused, or no answer to say. */
export type SendResult = "sent" | "not_sent" | "unknown"

/**
 * A turn as the conversation shows it now: not there (still on its way, or
 * gone past what the window holds), waiting or at work, or ended with what
 * the agent said.
 */
export type TurnState =
  | { readonly kind: "absent" }
  | { readonly kind: "open" }
  | { readonly kind: "ended"; readonly reply: AgentReply }

/** A message sent, and the turn it started, named by that message. */
export interface Sent {
  readonly result: SendResult
  readonly turn: string
}

/**
 * The conversation a call talks to, through the same commands the composer
 * uses: a message sent here is one the person typed. Whether a turn has
 * ended, and what it said, is the conversation's to say, by the turn's own
 * name, never guessed from its text or from when the chat went quiet.
 */
export interface VoiceConversation {
  /** Sends `text` as the person's message. */
  send(text: string): Promise<Sent>
  /** Stops the running turn, and withdraws the queued ones; whether there was one. */
  stop(): Promise<boolean>
  /** The latest turns, as plain text for the voice's context. */
  recentTurns(): string
  /** Where `turn` stands in the conversation the window holds now. */
  turnState(turn: string): TurnState
  /** Each time the conversation changes. Answers how to stop listening. */
  onChange(listener: () => void): () => void
}

/** The window's conversations, by session id. */
export type VoiceConversations = (sessionId: string) => VoiceConversation

/** Timers, so tests can run the transcript wait without waiting. */
export interface VoiceTimers {
  setTimeout(run: () => void, ms: number): unknown
  clearTimeout(handle: unknown): void
}
