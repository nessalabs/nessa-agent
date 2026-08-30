import * as React from "react"
import { Mic, Plus, Square } from "lucide-react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
  ChatTypingIndicator,
} from "@nessa-ui/react/chat-bubbles"
import {
  ChatComposerAction,
  ChatComposerInput,
} from "@nessa-ui/react/chat-composer"
import {
  MessageStreamText,
} from "@nessa-ui/react/message"
import {
  PillComposer,
  PillComposerRow,
} from "@nessa-ui/react/pill-composer"
import {
  RandomAvatar,
} from "@nessa-ui/react/random-avatar"

import { AGENT_HUES, AGENT_SEED } from "./agent-identity"
import {
  onFocusComposer,
  onToggleSurface,
  setFrosted,
  startResizeFromLeftEdge,
} from "./host-window"
import { useColorScheme } from "./use-color-scheme"
import { useSurface } from "./use-surface"

/** How long the typing dots hold before the reply starts arriving. */
const THINKING_MS = 800
/** How long a reply streams before it settles into a plain bubble. */
const STREAMING_MS = 2200

interface Turn {
  id: string
  from: "user" | "assistant"
  text: string
}

/** `thinking` shows the typing dots; `streaming` reveals the reply. */
type Phase = "idle" | "thinking" | "streaming"

/**
 * Stands in for the agent runtime that has not been wired up yet, so the
 * transcript, the typing dots, the streaming reveal, and the composer's lit
 * rim are all driven by the same state the runtime will drive.
 */
function draftReply(prompt: string) {
  return `You said "${prompt}". There is no agent behind this window yet — this is Nessa's chat UI and pill composer running in a floating Tauri panel.`
}

export function App() {
  const scheme = useColorScheme()
  const ground = scheme === "dark" ? "ink" : "paper"
  const [surface, toggleSurface] = useSurface()
  const [message, setMessage] = React.useState("")
  const [turns, setTurns] = React.useState<Turn[]>([])
  const [phase, setPhase] = React.useState<Phase>("idle")
  const pending = React.useRef("")
  const nextId = React.useRef(0)
  const logRef = React.useRef<HTMLDivElement>(null)
  const composerRef = React.useRef<HTMLTextAreaElement>(null)

  // The dots give way to the reply, which streams and then settles. Both legs
  // are timers here; a real runtime replaces them with stream events.
  React.useEffect(() => {
    if (phase === "idle") return

    if (phase === "thinking") {
      const timer = window.setTimeout(() => {
        setTurns((current) => [
          ...current,
          {
            id: `t${nextId.current++}`,
            from: "assistant",
            text: draftReply(pending.current),
          },
        ])
        setPhase("streaming")
      }, THINKING_MS)
      return () => window.clearTimeout(timer)
    }

    const timer = window.setTimeout(() => setPhase("idle"), STREAMING_MS)
    return () => window.clearTimeout(timer)
  }, [phase])

  // The frosted surface is half CSS and half a native window effect, so the
  // host has to be told whenever the choice changes — including on first paint,
  // when the remembered choice may not be the default the host started with.
  React.useEffect(() => {
    void setFrosted(surface === "translucent")
  }, [surface])

  // Summoning the panel — from the tray or the global shortcut — hands the
  // caret straight to the composer, so it can be typed into without a click.
  React.useEffect(() => {
    const subscription = onFocusComposer(() => composerRef.current?.focus())
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [])

  // The surface control lives in the tray menu; the panel only reacts to it.
  React.useEffect(() => {
    const subscription = onToggleSurface(toggleSurface)
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [toggleSurface])

  // The newest message stays in view as the transcript grows past the panel.
  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [turns, phase])

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const prompt = message.trim()
    if (prompt === "" || phase !== "idle") return

    pending.current = prompt
    setTurns((current) => [
      ...current,
      { id: `t${nextId.current++}`, from: "user", text: prompt },
    ])
    setMessage("")
    setPhase("thinking")
  }

  const generating = phase !== "idle"
  const streamingId = phase === "streaming" ? turns.at(-1)?.id : undefined

  return (
    <div className="flex h-full">
      <div
        data-nessa-root
        data-surface={surface}
        className="nessa-panel relative flex min-h-0 flex-1 flex-col overflow-hidden rounded-[18px] border"
      >
        {/* The window is undecorated, so macOS gives it no resize border of its
            own; this edge is the grab handle. */}
        <div
          role="presentation"
          onPointerDown={() => void startResizeFromLeftEdge()}
          className="absolute inset-y-0 left-0 z-10 w-1.5 cursor-ew-resize"
        />
        <header
          data-tauri-drag-region
          className="flex shrink-0 items-center gap-2 px-3 py-2.5"
        >
          <RandomAvatar
            seed={AGENT_SEED}
            hues={AGENT_HUES}
            name="Nessa"
            ground={ground}
            // The painting only moves while `busy`, so idle keeps it alive
            // rather than freezing between replies; working floods harder.
            busy
            speed={generating ? 2.4 : 1.6}
            flood={generating ? 1 : 0.75}
            animateOnMount
            // `busy` would otherwise mark the avatar mid-update whenever it is
            // merely breathing. Spread last, so this wins.
            aria-busy={generating || undefined}
            className="size-6 rounded-full"
          />
          <span className="nessa-text-3 font-medium text-foreground">Nessa</span>
        </header>

        <div
          ref={logRef}
          role="log"
          aria-label="Conversation"
          className="flex min-h-0 flex-1 select-text flex-col gap-5 overflow-y-auto px-3 pb-4 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          {/* Bottom-anchors a short transcript without justify-end, which
              would trap overflowing messages above an unscrollable top. */}
          <div aria-hidden="true" className="mt-auto shrink-0" />
          {turns.length === 0 && phase === "idle" ? (
            <EmptyState ground={ground} />
          ) : null}
          {turns.map((turn) => (
            <ChatMessage
              key={turn.id}
              tone={turn.from === "user" ? "sent" : "received"}
            >
              <ChatBubble>
                {turn.id === streamingId ? (
                  <MessageStreamText text={turn.text} />
                ) : (
                  turn.text
                )}
              </ChatBubble>
              {turn.from === "user" ? (
                <ChatMessageActions>
                  <ChatMessageReceipt>Delivered</ChatMessageReceipt>
                </ChatMessageActions>
              ) : null}
            </ChatMessage>
          ))}
          {phase === "thinking" ? (
            <ChatTypingIndicator label="Nessa is typing" />
          ) : null}
        </div>

        <div className="shrink-0 px-2.5 pb-2.5">
          <PillComposer generating={generating} onSubmit={submit}>
            <PillComposerRow>
              <ChatComposerAction aria-label="Add attachment" title="Add attachment">
                <Plus aria-hidden="true" />
              </ChatComposerAction>
              <ChatComposerInput
                value={message}
                onChange={(event) => setMessage(event.target.value)}
                placeholder="Ask me anything"
                className="self-center"
                autoFocus
              />
              {/* Enter is the send affordance, as the pill intends, so the
                  trailing slot carries voice instead — and hands over to the
                  way to stop a reply while one is arriving. Voice is inert
                  until there is a runtime to transcribe into. */}
              {generating ? (
                <ChatComposerAction
                  aria-label="Stop generating"
                  title="Stop generating"
                  onClick={() => setPhase("idle")}
                >
                  <Square aria-hidden="true" className="fill-current" />
                </ChatComposerAction>
              ) : (
                <ChatComposerAction
                  aria-label="Start voice input"
                  title="Start voice input"
                >
                  <Mic aria-hidden="true" />
                </ChatComposerAction>
              )}
            </PillComposerRow>
          </PillComposer>
        </div>
      </div>
    </div>
  )
}

function EmptyState({ ground }: { ground: "paper" | "ink" }) {
  return (
    <div className="flex flex-col items-center gap-2.5 py-6 text-center">
      <RandomAvatar
        seed={AGENT_SEED}
        hues={AGENT_HUES}
        name="Nessa"
        ground={ground}
        busy
        speed={1.6}
        flood={0.75}
        animateOnMount
        aria-busy={undefined}
        className="size-14 rounded-full"
      />
      <p className="nessa-text-3 m-0 text-muted-foreground">
        Nessa is listening. Press Enter to send.
      </p>
    </div>
  )
}
