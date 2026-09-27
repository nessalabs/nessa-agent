/**
 * A dev-only look at the real `Transcript` at panel width, served at
 * /transcript-preview.html by `pnpm dev`. Scratch: not part of any bundle.
 */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

import { makeStore } from "./store"
import { Transcript } from "./conversation"

/** The barrel is the door; a preview has no more business past it than the app. */
type Conversation = React.ComponentProps<typeof Transcript>["conversation"]

const tools = [
  {
    executionId: "run",
    toolId: "one",
    title: "Read",
    kind: "read",
    status: "completed",
    input: '{ "path": "src/panel/ui/app.tsx" }',
    details: "export function App() { …",
  },
  {
    executionId: "run",
    toolId: "two",
    title: "Search",
    kind: "search",
    status: "failed",
    input: '{ "pattern": "wrapTab" }',
    details: "No matches",
  },
  {
    executionId: "run",
    toolId: "three",
    title: "Search",
    kind: "search",
    status: "completed",
    input: '{ "pattern": "renderTab" }',
    details: "2 matches",
  },
  {
    executionId: "run",
    toolId: "four",
    title: "Edit",
    kind: "edit",
    status: "completed",
    input: '{ "path": "src/panel/ui/app.tsx" }',
    details: "1 change applied",
  },
  {
    executionId: "run",
    toolId: "five",
    title: "Shell",
    kind: "execute",
    status: "completed",
    input: '{ "command": "pnpm test" }',
    details: "26 files passed",
  },
]

const parts = [
  {
    offset: 0,
    kind: "thought" as const,
    text: "The tab bar is in the panel.",
    toolId: "",
  },
  { offset: 1, kind: "tool" as const, text: "", toolId: "one" },
  {
    offset: 2,
    kind: "text" as const,
    text: "Let me find where a tab is wrapped.",
    toolId: "",
  },
  { offset: 3, kind: "tool" as const, text: "", toolId: "two" },
  { offset: 4, kind: "thought" as const, text: "\n\n", toolId: "" },
  {
    offset: 5,
    kind: "thought" as const,
    text: "Wrong name — try `renderTab`.",
    toolId: "",
  },
  { offset: 6, kind: "tool" as const, text: "", toolId: "three" },
  { offset: 7, kind: "tool" as const, text: "", toolId: "four" },
  { offset: 8, kind: "tool" as const, text: "", toolId: "five" },
]

function chat(
  id: string,
  extra: Partial<Conversation>,
  agent: { text: string; status: string; parts: typeof parts },
  calls: typeof tools = tools,
): Conversation {
  return {
    id,
    title: "Tabs",
    phase: "idle",
    draft: [],
    turns: [
      {
        id: `${id}:user`,
        from: "user",
        executionId: "run",
        receipt: "delivered",
        content: [{ type: "text", text: "Why does the tab label clip at panel width?" }],
      },
      { id: `${id}:agent`, from: "assistant", executionId: "run", ...agent },
    ],
    remote: {
      running: false,
      tools: calls.map((tool) => ({ ...tool })),
      permissions: [],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
      },
      queueComplete: true,
      truncated: false,
    },
    ...extra,
  } as Conversation
}

const done = chat(
  "done",
  {},
  {
    text: "It clips because the label is measured before the rail collapses.",
    status: "completed",
    parts: [
      ...parts,
      {
        offset: 9,
        kind: "text" as const,
        text: "It clips because the label is measured before the rail collapses.",
        toolId: "",
      },
    ],
  },
)
const running = chat(
  "running",
  {},
  { text: "", status: "running", parts: parts.slice(0, 4) },
)
const single = chat(
  "single",
  {},
  {
    text: "That file is not there.",
    status: "completed",
    parts: [
      { offset: 0, kind: "tool" as const, text: "", toolId: "two" },
      { offset: 1, kind: "text" as const, text: "That file is not there.", toolId: "" },
    ],
  },
)
const thinking = chat(
  "thinking",
  {},
  { text: "", status: "running", parts: parts.slice(0, 1) },
)

/** Many calls, so the sheet has to pass its floor and actually move. */
const manyTools = Array.from({ length: 12 }, (_, index) => ({
  ...tools[index % tools.length],
  toolId: `many-${index}`,
  status: "completed",
}))

/** A turn that gains a tool every second, so the sheet's growth is watchable. */
function Growing({ label }: { label: string }) {
  const [count, setCount] = React.useState(1)
  React.useEffect(() => {
    if (count >= manyTools.length) return
    const timer = setTimeout(() => setCount((count) => count + 1), 1200)
    return () => clearTimeout(timer)
  }, [count])
  const conversation = chat(
    "growing",
    {},
    {
      text: "",
      status: "completed",
      parts: manyTools.slice(0, count).map((tool, index) => ({
        offset: index,
        kind: "tool" as const,
        text: "",
        toolId: tool.toolId,
      })),
    },
    manyTools,
  )
  return <Panel label={label} conversation={conversation} />
}

function Panel({ label, conversation }: { label: string; conversation: Conversation }) {
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs text-muted-foreground">{label}</p>
      <div className="flex h-[32rem] w-[420px] flex-col overflow-hidden rounded-[1.5rem] border border-border bg-background">
        <Transcript
          conversation={conversation}
          ground="paper"
          animateMount={false}
          streamText={false}
          emptyState={false}
          statusLabel=""
          gatewayAvailable
          onOpenPaste={() => {}}
        />
      </div>
    </div>
  )
}

const container = document.getElementById("root")
if (!container) throw new Error("preview root missing")

createRoot(container).render(
  <React.StrictMode>
    <Provider store={makeStore()}>
      <div className="flex flex-wrap gap-8 bg-muted p-8">
        <Panel label="Finished turn — one line for five tools" conversation={done} />
        <Panel label="While it runs" conversation={running} />
        <Panel label="Thinking, before any tool" conversation={thinking} />
        <Panel label="One tool — the sheet still has a floor" conversation={single} />
        <Growing label="A tool a second — open the sheet and watch it grow" />
      </div>
    </Provider>
  </React.StrictMode>,
)
