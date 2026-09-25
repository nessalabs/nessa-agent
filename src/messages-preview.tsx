/**
 * A dev-only look at the panel's Messages tab with made-up history, served at
 * /messages-preview.html by `pnpm dev`. The real panel needs a signed-in
 * gateway before it holds any conversations, and scenario mode never
 * fabricates one, so this composes the same pieces the panel does — the tab
 * strip, the conversation list, the transcript, the panel's own classes — over
 * fixed conversations. Size the window to the panel (about 420 wide) to see it
 * as it ships. Scratch: not part of any bundle.
 */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { ChatTabs, type ChatTabItem } from "@nessa-ui/react/chat-tabs"
import { ChatComposerInput } from "@nessa-ui/react/chat-composer"
import { PillComposer, PillComposerRow } from "@nessa-ui/react/pill-composer"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { makeStore, type StoreDependencies } from "./store"
// A dev preview composes the panel's own pieces around fixed conversations, so
// it reaches the Messages tab's identity and icon where the panel keeps them
// rather than restating either.
import { MESSAGES_TAB_ID as MESSAGES, stripOrder } from "./panel/application/messages-tab"
import { useMessagesTab } from "./panel/ui/use-messages-tab"
import { MessagesIcon } from "./panel/ui/messages-icon"
import {
  AGENT_HUES,
  commandErrorCleared,
  ConversationList,
  Transcript,
} from "./conversation"

/** Conversation types come through the barrel, as the app's do. */
type Conversation = React.ComponentProps<typeof Transcript>["conversation"]
type Turn = Conversation["turns"][number]

function said(id: string, user: string, reply?: string): Turn[] {
  const prompt: Turn = {
    id: `${id}:user`,
    from: "user",
    executionId: id,
    receipt: "delivered",
    content: [{ type: "text", text: user }],
  }
  if (reply === undefined) return [prompt]
  return [
    prompt,
    {
      id: `${id}:agent`,
      from: "assistant",
      executionId: id,
      status: "completed",
      text: reply,
      parts: [{ offset: 0, kind: "text", text: reply, toolId: "", noticeId: "" }],
    },
  ]
}

function chat(id: string, title: string, turns: Turn[]): Conversation {
  return { id, title, phase: "idle", draft: [], turns }
}

const history: Conversation[] = [
  chat("trip", "Lisbon trip", [
    ...said(
      "trip-1",
      "Can you find flights to Lisbon for the second week of October?",
      "Found three nonstop options from SFO. The cheapest is **TAP 238** on Oct 9 — $612 round trip, back on the 16th.",
    ),
    ...said(
      "trip-2",
      "Book the TAP one",
      "Holding it now. Should I use the passport name from your last booking?",
    ),
    ...said(
      "trip-3",
      "Yes, same as last time 👍",
      "Done. Confirmation **K7Q2PX** is in your inbox, and the flight is on your calendar.",
    ),
  ]),
  chat("tabs", "Tab label clipping", [
    ...said(
      "tabs-1",
      "Why does the tab label clip at panel width?",
      "It's measured before the rail collapses, so it keeps the wider width.",
    ),
  ]),
  chat("groceries", "Groceries", [
    ...said(
      "groceries-1",
      "Add oat milk, eggs and basil to the list",
      "Added all three. You now have 11 items for Saturday.",
    ),
    ...said("groceries-2", "And lemons"),
  ]),
  chat("standup", "Standup notes", [
    ...said(
      "standup-1",
      "Summarise yesterday's standup",
      "- Install audit landed\n- Delivery payload sizes fixed\n- Next: the Messages tab",
    ),
  ]),
  // Opened and never written in: a tab, but not a row in the list.
  chat("blank", "New chat", []),
]

/**
 * The gateway, as far as the Messages list asks it anything: a list of every
 * conversation, one of them closed — held by no tab here — to show that the
 * list is the gateway's and not the tab strip's. Titles and previews are
 * written the way the gateway derives them.
 */
const listed = [
  ...history
    .flatMap((item) => {
      const last = item.turns.at(-1)
      return last ? [{ item, last }] : []
    })
    .map(({ item, last }, index) => {
      return {
        conversationId: item.id,
        title: item.title,
        preview: (last.from === "assistant"
          ? last.text.replace(/\*\*|^[-#] /gm, "")
          : last.content.map((part) => ("text" in part ? part.text : "")).join("")
        )
          .replace(/\s+/g, " ")
          .trim(),
        updatedAtMs: 10 - index,
        running: false,
      }
    }),
  {
    conversationId: "reading",
    title: "What was that essay on calm",
    preview:
      "The Coming Age of Calm Technology by Mark Weiser and John Seely Brown, 1996.",
    updatedAtMs: 1,
    running: false,
  },
]

/** What the preview's gateway has archived or deleted, so a swipe does something. */
const archivedIds = new Set<string>()
const deletedIds = new Set<string>()

const previewStore = makeStore({
  conversation: {
    list: async (archived: boolean) => ({
      conversations: listed
        .filter((row) => !deletedIds.has(row.conversationId))
        .filter((row) => archivedIds.has(row.conversationId) === archived)
        .map((row) => ({ ...row, archived })),
      complete: true,
    }),
    archive: async (id: string, archived: boolean) => {
      const changed = archivedIds.has(id) !== archived
      if (archived) archivedIds.add(id)
      else archivedIds.delete(id)
      return changed
    },
    delete: async (id: string) => {
      deletedIds.add(id)
    },
  } as unknown as StoreDependencies["conversation"],
  attachments: { retain: () => {} } as unknown as StoreDependencies["attachments"],
  canChoosePaths: false,
})

function Panel() {
  const dark = window.matchMedia("(prefers-color-scheme: dark)").matches
  React.useLayoutEffect(() => {
    document.documentElement.classList.toggle("dark", dark)
  }, [dark])
  const ground = dark ? "ink" : "paper"
  const [conversations, setConversations] = React.useState(history)
  const [activeId, setActiveId] = React.useState("trip")
  // The panel's own Messages-tab rules, and what leaving the tab lets go of.
  const { messages, move } = useMessagesTab(() =>
    previewStore.dispatch(commandErrorCleared()),
  )
  React.useEffect(() => move("show"), [move])
  const active = conversations.find((item) => item.id === activeId) ?? conversations[0]

  function show(id: string) {
    if (id === MESSAGES) return move("show")
    move("leave")
    setActiveId(id)
  }

  function openNew() {
    const id = `new-${Date.now()}`
    setConversations((all) => [...all, chat(id, "New chat", [])])
    show(id)
  }

  const conversationTabs: ChatTabItem[] = conversations.map((item) => ({
    id: item.id,
    title: item.title,
    closeable: true,
    icon: (
      <RandomAvatar
        seed={item.id}
        hues={AGENT_HUES}
        ground={ground}
        className="size-5 rounded-full"
      />
    ),
  }))
  const messagesTab: ChatTabItem = {
    id: MESSAGES,
    title: "Messages",
    kind: "history",
    closeable: true,
    icon: <MessagesIcon className="nessa-messages-icon" />,
  }
  // In the order the panel's strip puts them.
  const byId = new Map(conversationTabs.map((tab) => [tab.id, tab]))
  const tabs = stripOrder(
    messages,
    conversations.map((item) => item.id),
    null,
  ).flatMap((id) => {
    const tab = id === MESSAGES ? messagesTab : byId.get(id)
    return tab ? [tab] : []
  })

  return (
    <div className="nessa-stage" data-host="browser">
      <div
        data-nessa-root
        data-surface="translucent"
        data-host="browser"
        className="nessa-panel relative flex min-h-0 flex-col overflow-hidden rounded-[18px] border"
      >
        <div className="nessa-chrome shrink-0 px-2 pt-2 pb-1">
          <ChatTabs
            label="Conversations"
            tabs={tabs}
            value={messages === "viewing" ? MESSAGES : active.id}
            onValueChange={show}
            onClose={(id) => {
              if (id === MESSAGES) return move("close")
              setConversations((all) => all.filter((item) => item.id !== id))
            }}
            onNew={openNew}
            newTabLabel="New conversation"
            trailing={
              <button
                type="button"
                aria-label="Messages"
                title="Messages"
                aria-pressed={messages === "viewing"}
                onClick={() => show(MESSAGES)}
                className="nessa-messages-open"
              >
                <MessagesIcon className="nessa-messages-icon" />
              </button>
            }
          />
        </div>
        <div className="nessa-transcript-region relative flex min-h-0 flex-1 flex-col">
          {messages === "viewing" ? (
            <div className="nessa-messages-tab" role="tabpanel" aria-label="Messages">
              <div className="nessa-messages-pane">
                <ConversationList
                  onSelect={(target) => {
                    if ("tabId" in target) return
                    const id = target.serverConversationId
                    if (!conversations.some((item) => item.id === id))
                      setConversations((all) => [
                        ...all,
                        chat(id, target.title ?? "Conversation", []),
                      ])
                    show(id)
                  }}
                  onNew={openNew}
                />
              </div>
            </div>
          ) : (
            <Transcript
              key={active.id}
              conversation={active}
              ground={ground}
              animateMount
              streamText={false}
              emptyState
              statusLabel=""
              gatewayAvailable
              onOpenPaste={() => {}}
            />
          )}
        </div>
        <div className="nessa-composer" hidden={messages === "viewing" || undefined}>
          <PillComposer generating={false}>
            <PillComposerRow>
              <ChatComposerInput placeholder="Ask me anything" aria-label="Message" />
            </PillComposerRow>
          </PillComposer>
        </div>
      </div>
    </div>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("preview root missing")

/** `?icons`: the Messages icon at the sizes it is drawn at, and recoloured. */
function Icons() {
  return (
    <div className="flex flex-col gap-8 bg-background p-8">
      <div className="flex items-end gap-6">
        {[256, 128, 64, 32, 20, 16].map((size) => (
          <MessagesIcon key={size} style={{ width: size, height: size }} />
        ))}
      </div>
      <div className="flex items-end gap-6">
        <MessagesIcon
          style={{ width: 128, height: 128 }}
          colors={{
            deep: "#7a2cff",
            mid: "#b05cff",
            lavender: "#f28bd2",
            light: "#fbd0ee",
            wave: "#ffb3c7",
            foam: "#ffe0e8",
          }}
        />
        <MessagesIcon
          style={{ width: 128, height: 128 }}
          colors={{
            deep: "#0e8f6a",
            mid: "#23b58a",
            lavender: "#6fd3c1",
            light: "#c9f2e6",
            wave: "#9be7c9",
            foam: "#dcf8ec",
          }}
        />
      </div>
    </div>
  )
}

createRoot(container).render(
  <React.StrictMode>
    <Provider store={previewStore}>
      {new URLSearchParams(window.location.search).has("icons") ? <Icons /> : <Panel />}
    </Provider>
  </React.StrictMode>,
)
