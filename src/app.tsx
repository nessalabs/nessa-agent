import * as React from "react"
import { Plus, Square } from "lucide-react"
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
import { ChatTabs, type ChatTabItem } from "@nessa-ui/react/chat-tabs"
import { MessageStreamText } from "@nessa-ui/react/message"
import { PillComposer, PillComposerRow } from "@nessa-ui/react/pill-composer"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES } from "./agent-identity"
import {
  onFocusComposer,
  onToggleSurface,
  setFrosted,
  startResizeFromLeftEdge,
} from "./host-window"
import { useColorScheme } from "./use-color-scheme"
import { useSurface } from "./use-surface"
import { WaveformIcon } from "./waveform-icon"

/** How long the typing dots hold before the reply starts arriving. */
const THINKING_MS = 800
/** How long a reply streams before it settles into a plain bubble. */
const STREAMING_MS = 2200
/** A conversation is named after its opening line, cut to this. */
const TITLE_LENGTH = 24

interface Turn {
  id: string
  from: "user" | "assistant"
  text: string
}

/** `thinking` shows the typing dots; `streaming` reveals the reply. */
type Phase = "idle" | "thinking" | "streaming"

interface Conversation {
  id: string
  title: string
  turns: Turn[]
  phase: Phase
  /** The prompt awaiting a reply, held while the dots are up. */
  pending: string
  /** Drafts belong to a conversation, not to the composer: switching tabs
   *  mid-sentence must not carry the sentence into someone else's thread. */
  draft: string
}

function newConversation(id: string): Conversation {
  return { id, title: "New chat", turns: [], phase: "idle", pending: "", draft: "" }
}

/**
 * Stands in for the agent runtime that has not been wired up yet, so the
 * transcript, the typing dots, the streaming reveal, and the composer's lit
 * rim are all driven by the same state the runtime will drive.
 */
function draftReply(prompt: string) {
  return `You said "${prompt}". There is no agent behind this window yet — this is Nessa's chat UI and pill composer running in a floating Tauri panel.`
}

function titleFor(prompt: string) {
  const trimmed = prompt.trim()
  return trimmed.length > TITLE_LENGTH
    ? `${trimmed.slice(0, TITLE_LENGTH).trimEnd()}…`
    : trimmed
}

export function App() {
  const scheme = useColorScheme()
  const ground = scheme === "dark" ? "ink" : "paper"
  const [surface, toggleSurface] = useSurface()
  const [conversations, setConversations] = React.useState<Conversation[]>(() => [
    newConversation("c0"),
  ])
  const [activeId, setActiveId] = React.useState("c0")
  const nextId = React.useRef(1)
  const logRef = React.useRef<HTMLDivElement>(null)
  const composerRef = React.useRef<HTMLTextAreaElement>(null)

  // A closed tab can briefly leave `activeId` pointing at nothing.
  const active =
    conversations.find((conversation) => conversation.id === activeId) ??
    conversations[0]!

  function update(id: string, change: (conversation: Conversation) => Conversation) {
    setConversations((current) =>
      current.map((conversation) =>
        conversation.id === id ? change(conversation) : conversation,
      ),
    )
  }

  /** Moves one conversation to its next phase, wherever it is in the strip. */
  function advance(id: string) {
    update(id, (conversation) => {
      if (conversation.phase === "thinking") {
        return {
          ...conversation,
          phase: "streaming",
          turns: [
            ...conversation.turns,
            {
              id: `t${nextId.current++}`,
              from: "assistant",
              text: draftReply(conversation.pending),
            },
          ],
        }
      }
      return { ...conversation, phase: "idle" }
    })
  }

  // The newest message stays in view as the transcript grows past the panel.
  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [active.turns, active.phase, activeId])

  // Summoning the panel — from the tray or the global shortcut — hands the
  // caret straight to the composer, so it can be typed into without a click.
  React.useEffect(() => {
    const subscription = onFocusComposer(() => composerRef.current?.focus())
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [])

  React.useEffect(() => {
    void setFrosted(surface === "translucent")
  }, [surface])

  // The surface control lives in the tray menu; the panel only reacts to it.
  React.useEffect(() => {
    const subscription = onToggleSurface(toggleSurface)
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [toggleSurface])

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const prompt = active.draft.trim()
    if (prompt === "" || active.phase !== "idle") return

    update(active.id, (conversation) => ({
      ...conversation,
      // The opening line names the conversation; later ones do not rename it.
      title: conversation.turns.length === 0 ? titleFor(prompt) : conversation.title,
      turns: [
        ...conversation.turns,
        { id: `t${nextId.current++}`, from: "user", text: prompt },
      ],
      pending: prompt,
      phase: "thinking",
      draft: "",
    }))
  }

  function openConversation() {
    const id = `c${nextId.current++}`
    setConversations((current) => [...current, newConversation(id)])
    setActiveId(id)
    composerRef.current?.focus()
  }

  function closeConversation(id: string) {
    setConversations((current) => {
      // The strip is the app's only navigation, so it never empties.
      if (current.length === 1) return [newConversation(`c${nextId.current++}`)]
      const remaining = current.filter((conversation) => conversation.id !== id)
      if (id === activeId) {
        const closed = current.findIndex((conversation) => conversation.id === id)
        setActiveId((remaining[closed] ?? remaining[remaining.length - 1]!).id)
      }
      return remaining
    })
  }

  const generating = active.phase !== "idle"
  const streamingId = active.phase === "streaming" ? active.turns.at(-1)?.id : undefined

  const tabs: ChatTabItem[] = conversations.map((conversation) => ({
    id: conversation.id,
    title: conversation.title,
    closeable: true,
    // The avatar breathes constantly, so it cannot double as the working
    // signal the way a subagent's does in the design system's own story —
    // the dot stays on to say which thread is actually mid-reply.
    loading: conversation.phase !== "idle",
    icon: (
      <RandomAvatar
        seed={conversation.id}
        hues={AGENT_HUES}
        ground={ground}
        busy
        speed={conversation.phase === "idle" ? 1.6 : 2.4}
        flood={conversation.phase === "idle" ? 0.75 : 1}
        aria-busy={undefined}
        className="size-4 rounded-full"
      />
    ),
  }))

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

        {/* The strip is the titlebar too: the gaps around the tabs drag the
            window, while the tabs themselves stay clickable. */}
        <div data-tauri-drag-region className="shrink-0 px-1.5 pt-2 pb-1">
          <ChatTabs
            label="Conversations"
            tabs={tabs}
            value={active.id}
            onValueChange={setActiveId}
            onClose={closeConversation}
            onNew={openConversation}
            newTabLabel="New conversation"
          />
        </div>

        <div
          ref={logRef}
          role="log"
          aria-label={`${active.title} transcript`}
          className="flex min-h-0 flex-1 select-text flex-col gap-5 overflow-y-auto px-3 pb-4 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          {/* Bottom-anchors a short transcript without justify-end, which
              would trap overflowing messages above an unscrollable top. */}
          <div aria-hidden="true" className="mt-auto shrink-0" />
          {active.turns.length === 0 && active.phase === "idle" ? (
            <EmptyState seed={active.id} ground={ground} />
          ) : null}
          {active.turns.map((turn) => (
            <ChatMessage key={turn.id} tone={turn.from === "user" ? "sent" : "received"}>
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
          {active.phase === "thinking" ? (
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
                ref={composerRef}
                value={active.draft}
                onChange={(event) =>
                  update(active.id, (conversation) => ({
                    ...conversation,
                    draft: event.target.value,
                  }))
                }
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
                  onClick={() => update(active.id, (c) => ({ ...c, phase: "idle" }))}
                >
                  <Square aria-hidden="true" className="fill-current" />
                </ChatComposerAction>
              ) : (
                <ChatComposerAction
                  aria-label="Start voice input"
                  title="Start voice input"
                >
                  <WaveformIcon className="size-[18px]" />
                </ChatComposerAction>
              )}
            </PillComposerRow>
          </PillComposer>
        </div>
      </div>

      {/* One timer per conversation, so a reply arriving in one thread does
          not restart the clock on another's. */}
      {conversations.map((conversation) => (
        <ReplyTimer
          key={conversation.id}
          phase={conversation.phase}
          onAdvance={() => advance(conversation.id)}
        />
      ))}
    </div>
  )
}

/**
 * The stand-in runtime's clock for one conversation. Owning it per
 * conversation is what keeps background threads streaming on their own
 * schedule; a single shared effect would reset every timer whenever any one
 * of them advanced.
 */
function ReplyTimer({ phase, onAdvance }: { phase: Phase; onAdvance: () => void }) {
  const latest = React.useRef(onAdvance)
  React.useEffect(() => {
    latest.current = onAdvance
  })

  React.useEffect(() => {
    if (phase === "idle") return
    const delay = phase === "thinking" ? THINKING_MS : STREAMING_MS
    const timer = window.setTimeout(() => latest.current(), delay)
    return () => window.clearTimeout(timer)
  }, [phase])

  return null
}

function EmptyState({ seed, ground }: { seed: string; ground: "paper" | "ink" }) {
  return (
    <div className="flex flex-col items-center gap-2.5 py-6 text-center">
      <RandomAvatar
        seed={seed}
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
