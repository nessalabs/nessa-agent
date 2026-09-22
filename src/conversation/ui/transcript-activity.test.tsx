// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { textContent, type AgentFeatures, type Conversation } from "../model"
import { Transcript } from "./transcript"

class TestResizeObserver {
  observe() {}
  disconnect() {}
}

const store = {
  dispatch: vi.fn(),
  getState: () => ({}),
  subscribe: () => () => {},
  replaceReducer: vi.fn(),
}

const agentFeatures: AgentFeatures = {
  permissionDenial: "unknown",
  nativeHookSuppression: "unknown",
  compactionReporting: "unsupported_not_implemented",
  modelSwitchReporting: "unsupported_not_implemented",
  permissionDeferral: "unsupported_not_implemented",
  elicitationForwarding: "unknown",
  preToolPolicy: "unsupported_not_implemented",
  policyEndTurn: "unsupported_not_implemented",
  policyCloseSession: "unsupported_not_implemented",
  incomingElicitation: "unsupported_not_implemented",
}

function workingConversation(id: string): Conversation {
  return {
    id,
    title: "Activity",
    phase: "streaming",
    pending: "submission",
    draft: [],
    turns: [
      {
        id: "user",
        from: "user",
        executionId: "run",
        receipt: "delivered",
        content: textContent("Inspect"),
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "",
        status: "running",
        parts: [
          { offset: 0, kind: "thought", text: "First thought", toolId: "" },
          { offset: 1, kind: "tool", text: "", toolId: "one" },
          { offset: 2, kind: "thought", text: "Second thought", toolId: "" },
          { offset: 3, kind: "tool", text: "", toolId: "two" },
        ],
      },
    ],
    remote: {
      running: true,
      permissions: [
        {
          executionId: "run",
          permissionId: "permission",
          toolId: "two",
          title: "Approve shell",
          toolName: "Shell",
          argumentsJson: "{}",
          options: [{ id: "allow", label: "Allow" }],
        },
      ],
      tools: [
        {
          executionId: "run",
          toolId: "one",
          title: "Search",
          status: "failed",
          input: "needle",
          details: "No matches",
        },
        {
          executionId: "run",
          toolId: "two",
          title: "Read",
          status: "running",
          input: "README.md",
          details: "Project notes",
        },
      ],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
        agentFeatures,
      },
      lifecycle: { phase: "attached" },
      queueComplete: false,
      truncated: false,
    },
  }
}

const withoutActivity = (value: Conversation): Conversation => ({
  ...value,
  phase: "idle",
  turns: value.turns.map((turn) =>
    turn.from === "assistant"
      ? { ...turn, status: "completed", text: "Done", parts: [] }
      : turn,
  ),
  remote: value.remote ? { ...value.remote, running: false, tools: [] } : undefined,
})

function terminalConversation(
  status: "failed" | "cancelled",
  parts: Extract<Conversation["turns"][number], { from: "assistant" }>["parts"],
  tools: NonNullable<Conversation["remote"]>["tools"] = [],
): Conversation {
  return {
    id: "terminal",
    title: "Terminal",
    phase: "thinking",
    pending: "submission",
    draft: [],
    turns: [
      {
        id: "user",
        from: "user",
        executionId: "run",
        receipt: "delivered",
        content: textContent("Inspect"),
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: parts
          .filter((part) => part.kind === "text")
          .map((part) => part.text)
          .join(""),
        status,
        parts,
      },
    ],
    remote: {
      running: false,
      permissions: [],
      tools,
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
        agentFeatures,
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      truncated: false,
    },
  }
}

function queuedConversation(): Conversation {
  return {
    id: "queued",
    title: "Queued",
    phase: "starting",
    pending: "submission",
    draft: [],
    turns: [
      {
        id: "queued-user",
        from: "user",
        executionId: "queued-run",
        receipt: "queued",
        content: textContent("Queued prompt"),
      },
    ],
    remote: {
      running: false,
      permissions: [],
      tools: [],
      pending: [
        {
          executionId: "queued-run",
          text: "Queued prompt",
          attachments: [],
          mode: "queued",
        },
      ],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: true,
        agentFeatures,
      },
      lifecycle: { phase: "starting" },
      queueComplete: true,
      truncated: false,
    },
  }
}

let container: HTMLDivElement
let root: Root

async function render(conversation: Conversation) {
  await React.act(async () => {
    root.render(
      <Provider store={store as never}>
        <Transcript
          conversation={conversation}
          ground="paper"
          animateMount={false}
          streamText={false}
          emptyState={false}
          statusLabel="Ready"
          gatewayAvailable
          onOpenPaste={vi.fn()}
        />
      </Provider>,
    )
  })
}

async function click(button: Element) {
  await React.act(async () => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }))
  })
}

async function waitForText(text: string) {
  const deadline = Date.now() + 2_000
  while (!document.body.textContent?.includes(text) && Date.now() < deadline) {
    await React.act(async () => new Promise((resolve) => setTimeout(resolve, 10)))
  }
  expect(document.body.textContent).toContain(text)
}

function requiredElement(parent: ParentNode, selector: string) {
  const element = parent.querySelector(selector)
  expect(element).not.toBeNull()
  if (!element) throw new Error(`Missing test element: ${selector}`)
  return element
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver = TestResizeObserver as unknown as typeof ResizeObserver
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })
  Element.prototype.animate = vi.fn(() => ({ cancel: vi.fn() })) as never
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("shows queued input before an assistant row without inventing activity or duplicating it later", async () => {
  const queued = queuedConversation()
  for (let snapshot = 0; snapshot < 2; snapshot++)
    await render({ ...queued, revision: String(snapshot) })

  let sent = container.querySelectorAll('[data-slot="chat-message"][data-tone="sent"]')
  expect(sent).toHaveLength(1)
  expect(sent.item(0).textContent).toContain("Queued prompt")
  expect(sent.item(0).textContent).toContain("Queued")
  expect(container.querySelector('[aria-label="Nessa is typing"]')).toBeNull()
  expect(container.querySelector('[data-slot="agent-activity"]')).toBeNull()

  const completed: Conversation = {
    id: queued.id,
    title: queued.title,
    phase: "idle",
    draft: [],
    turns: [
      {
        id: "queued-user",
        from: "user",
        executionId: "queued-run",
        receipt: "delivered",
        content: textContent("Queued prompt"),
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "queued-run",
        text: "Done",
        status: "completed",
        parts: [{ offset: 0, kind: "text", text: "Done", toolId: "" }],
      },
    ],
    remote: queued.remote
      ? {
          ...queued.remote,
          running: false,
          pending: [],
          lifecycle: { phase: "attached" },
        }
      : undefined,
  }
  await render(completed)

  sent = container.querySelectorAll('[data-slot="chat-message"][data-tone="sent"]')
  expect(sent).toHaveLength(1)
  expect(sent.item(0).textContent).toContain("Queued prompt")
  expect(container.textContent).toContain("Done")

  await render({
    ...completed,
    phase: "thinking",
    pending: "submission",
    turns: [
      ...completed.turns,
      {
        id: "follow-up",
        from: "user",
        executionId: "follow-up-run",
        receipt: "queued",
        content: textContent("Queued follow-up"),
      },
    ],
    remote: completed.remote
      ? {
          ...completed.remote,
          pending: [
            {
              executionId: "follow-up-run",
              text: "Queued follow-up",
              attachments: [],
              mode: "queued",
            },
          ],
        }
      : undefined,
  })

  const messages = [
    ...container.querySelectorAll<HTMLElement>('[data-slot="chat-message"]'),
  ]
  expect(messages.map((message) => message.dataset.tone)).toEqual([
    "sent",
    "received",
    "sent",
  ])
  expect(messages.at(-1)?.textContent).toContain("Queued follow-up")
  expect(messages.at(-1)?.textContent).toContain("Queued")
  expect(container.querySelector('[aria-label="Nessa is typing"]')).toBeNull()
  expect(container.querySelector('[data-slot="agent-activity"]')).toBeNull()
})

it("renders one turn activity, keeps details and permission controls reachable, and clears stale selection", async () => {
  const conversation = workingConversation("first")
  await render(conversation)

  const activities = container.querySelectorAll('[data-slot="agent-activity"]')
  expect(activities).toHaveLength(1)
  const trigger = requiredElement(activities.item(0), "button")
  expect(trigger.textContent).toBe("Running…")
  expect(container.querySelector('[aria-label="Approve Approve shell"]')).not.toBeNull()

  await click(trigger)
  await waitForText("First thought")
  expect(document.body.textContent).toContain("Second thought")
  expect(document.body.textContent).toContain("Search")
  expect(document.body.textContent).toContain("Read")

  const failedTool = [...document.body.querySelectorAll('[data-slot="tool-call"]')].find(
    (row) => row.textContent?.includes("Search"),
  )
  expect(failedTool).toBeDefined()
  if (!failedTool) throw new Error("Missing failed tool row")
  await click(requiredElement(failedTool, "button"))
  expect(document.body.textContent).toContain("needle")
  expect(document.body.textContent).toContain("No matches")
  expect(
    document.body.querySelector('[aria-label="Approve Approve shell"]'),
  ).not.toBeNull()

  // Both conversations deliberately reuse the turn, tool, and activity ids.
  // A selected segment belongs to the conversation that opened it, so an
  // immediate tab switch must not retarget the open sheet to the collision.
  await render(workingConversation("second"))
  expect(document.body.querySelector('[aria-label="Turn activity"]')).toBeNull()

  await render(conversation)
  await click(requiredElement(container, '[data-slot="agent-activity"] button'))
  await waitForText("First thought")
  await render(withoutActivity(conversation))
  await React.act(async () => {})
  expect(document.body.querySelector('[aria-label="Turn activity"]')).toBeNull()
})

it.each([
  ["failed", "failed"],
  ["cancelled", "Cancelled"],
] as const)(
  "renders one accessible %s notice for a terminal turn with no text",
  async (status, label) => {
    await render(terminalConversation(status, []))
    const notices = container.querySelectorAll('p[role="status"]')
    expect(notices).toHaveLength(1)
    expect(notices.item(0).textContent).toBe(label)
    expect(container.querySelector('[aria-label="Nessa is typing"]')).toBeNull()
  },
)

it("renders one terminal notice beside a tools-only turn", async () => {
  await render(
    terminalConversation(
      "failed",
      [{ offset: 0, kind: "tool", text: "", toolId: "shell" }],
      [
        {
          executionId: "run",
          toolId: "shell",
          title: "Shell",
          status: "failed",
          input: "false",
          details: "exit 1",
        },
      ],
    ),
  )
  expect(container.querySelectorAll('[data-slot="agent-activity"]')).toHaveLength(1)
  expect(container.querySelectorAll('p[role="status"]')).toHaveLength(1)
  expect(requiredElement(container, 'p[role="status"]').textContent).toBe("failed")
})

it("renders one terminal notice across multiple text rows and repeated renders", async () => {
  const conversation = terminalConversation("failed", [
    { offset: 0, kind: "text", text: "First.", messageId: "first", toolId: "" },
    { offset: 1, kind: "text", text: "Second.", messageId: "second", toolId: "" },
  ])
  for (let renderCount = 0; renderCount < 3; renderCount++) {
    await render({ ...conversation, revision: String(renderCount) })
    const notices = container.querySelectorAll('p[role="status"]')
    expect(notices).toHaveLength(1)
    expect(notices.item(0).textContent).toBe("failed")
  }
  expect(container.textContent).toContain("First.")
  expect(container.textContent).toContain("Second.")
})
