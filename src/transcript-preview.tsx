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
import { Transcript } from "./conversation/ui/transcript"
import { textContent } from "./conversation/model"
import type { Conversation } from "./conversation/model"

const tools = [
  {
    executionId: "run",
    toolId: "one",
    title: "Read",
    status: "completed",
    input: '{ "path": "src/panel/ui/app.tsx" }',
    details: "export function App() { …",
  },
  {
    executionId: "run",
    toolId: "two",
    title: "Search",
    status: "failed",
    input: '{ "pattern": "wrapTab" }',
    details: "No matches",
  },
  {
    executionId: "run",
    toolId: "three",
    title: "Search",
    status: "completed",
    input: '{ "pattern": "renderTab" }',
    details: "2 matches",
  },
  {
    executionId: "run",
    toolId: "four",
    title: "Edit",
    status: "completed",
    input: '{ "path": "src/panel/ui/app.tsx" }',
    details: "1 change applied",
  },
  {
    executionId: "run",
    toolId: "five",
    title: "Shell",
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
        content: textContent("Why does the tab label clip at panel width?"),
      },
      { id: `${id}:agent`, from: "assistant", executionId: "run", ...agent },
    ],
    remote: {
      running: false,
      tools: tools.map((tool) => ({ ...tool })),
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
const thinking = chat(
  "thinking",
  {},
  { text: "", status: "running", parts: parts.slice(0, 1) },
)

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
      </div>
    </Provider>
  </React.StrictMode>,
)
