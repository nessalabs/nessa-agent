/**
 * The window's conversations as a call sees them: the workspace store's own
 * `sendMessage` and `stopTurn`, so what the voice sends is what the composer
 * would, with the person as its initiator; and each turn's end, read from
 * the transcript the store already follows.
 */
import {
  atWork,
  type Transcript,
  type TurnMessage,
} from "../../workspace/model/transcript"
import { sendMessage, stopTurn } from "../../workspace/adapters/store/commands"
import { selectTranscript } from "../../workspace/adapters/store/selectors"
import { inputId, replyId } from "../../workspace/adapters/gateway/gateway-views"
import type { DesktopStore } from "../../store"
import type {
  TurnState,
  VoiceConversation,
  VoiceConversations,
} from "../application/ports"

/** How many of the latest turns the voice is given when a call starts. */
const CONTEXT_TURNS = 6
/** The most of one turn's text the context carries. */
const CONTEXT_CHARS = 600

function textOf(message: TurnMessage): string {
  return message.parts
    .flatMap((part) => (part.kind === "text" ? [part.text] : []))
    .join("\n")
    .trim()
}

function turnsOf(transcript: Transcript | undefined): TurnMessage[] {
  return (transcript?.messages ?? []).filter(
    (message): message is TurnMessage =>
      message.role === "user" || message.role === "agent",
  )
}

/**
 * Where the turn started by message `turn` stands, from the transcript:
 * ended once its reply is no longer being written or its end is marked; a
 * message answered by nothing at all, neither waiting for an agent nor with
 * the chat at work, ended saying nothing; absent when neither its message
 * nor its reply is in the transcript.
 */
export function turnState(transcript: Transcript | undefined, turn: string): TurnState {
  const turns = turnsOf(transcript)
  const reply = turns.find((message) => message.id === replyId(turn))
  if (reply?.working) return { kind: "open" }
  const marked =
    reply ?? turns.find((message) => message.terminal && message.executionId === turn)
  const input = turns.find((message) => message.id === inputId(turn))
  if (!marked) {
    if (!input) return { kind: "absent" }
    if (input.waiting || atWork(transcript)) return { kind: "open" }
  }
  const terminal = marked?.terminal
  return {
    kind: "ended",
    reply: {
      text: reply ? textOf(reply) : "",
      ended: !terminal
        ? "finished"
        : terminal.status === "cancelled"
          ? "stopped"
          : "failed",
    },
  }
}

/** The latest turns, one line each, for the voice's context. */
export function recentTurnsOf(transcript: Transcript | undefined): string {
  return turnsOf(transcript)
    .slice(-CONTEXT_TURNS)
    .map((message) => {
      const text = textOf(message).replace(/\s+/g, " ").slice(0, CONTEXT_CHARS)
      return text ? `${message.role === "user" ? "Person" : "Agent"}: ${text}` : ""
    })
    .filter((line) => line !== "")
    .join("\n")
}

export function storeConversations(store: DesktopStore): VoiceConversations {
  return (sessionId: string): VoiceConversation => ({
    async send(text) {
      const turn = crypto.randomUUID()
      const outcome = await store.dispatch(
        sendMessage({
          sessionId,
          text,
          initiator: "person",
          withImages: false,
          messageId: turn,
        }),
      )
      return {
        turn,
        result:
          outcome === "sent" ? "sent" : outcome === "unknown" ? "unknown" : "not_sent",
      }
    },
    async stop() {
      const outcome = await store.dispatch(stopTurn({ sessionId, initiator: "person" }))
      return outcome === "sent"
    },
    recentTurns: () => recentTurnsOf(selectTranscript(store.getState(), sessionId)),
    turnState: (turn) => turnState(selectTranscript(store.getState(), sessionId), turn),
    onChange(listener) {
      let last = selectTranscript(store.getState(), sessionId)
      return store.subscribe(() => {
        const transcript = selectTranscript(store.getState(), sessionId)
        if (transcript === last) return
        last = transcript
        listener()
      })
    },
  })
}
