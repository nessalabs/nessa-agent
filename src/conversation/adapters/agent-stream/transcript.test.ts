import { expect, it } from "vitest"
import { agentTranscript } from "./transcript"
import { textContent, type Turn } from "../../model"
import type { ConversationView } from "../../application/view"

const turns: Turn[] = [
  {
    id: "user",
    from: "user",
    executionId: "run",
    content: textContent("Inspect"),
    receipt: "delivered",
  },
  {
    id: "answer",
    from: "assistant",
    executionId: "run",
    text: "  Done.\n",
    thought: "  Inspect first.\n",
    parts: [
      { offset: 0, kind: "thought", text: "  Inspect first.\n", toolId: "" },
      { offset: 1, kind: "tool", text: "", toolId: "shell" },
      { offset: 2, kind: "text", text: "  Done.\n", toolId: "" },
    ],
    status: "completed",
  },
]
const tool: ConversationView["tools"][number] = {
  executionId: "run",
  toolId: "shell",
  title: "Shell",
  status: "completed",
  input: JSON.stringify({ command: 'echo "hi"' }),
  details: "  <output>\n",
}

it("maps gateway data through the shared builder without duplicating the final answer", () => {
  const transcript = agentTranscript("chat", turns, [tool])
  expect(transcript.turns).toHaveLength(1)
  expect(transcript.turns[0]?.finalText).toBe("  Done.\n")
  expect(transcript.turns[0]?.toolCalls).toBe(1)
  expect(
    transcript.turns[0]?.work.some(
      (event) => "payload" in event && event.payload.type === "assistant_text",
    ),
  ).toBe(false)
  expect(transcript.events.map((event) => event.payload.type)).toEqual([
    "user_message",
    "reasoning",
    "tool_call_started",
    "tool_call_completed",
    "assistant_text",
    "turn_completed",
  ])
  expect(transcript.events.every((event) => event.ts === null)).toBe(true)
  expect(transcript.events[2]?.payload).toMatchObject({ input: JSON.parse(tool.input) })
  expect([...transcript.resultByCallId.values()][0]?.text).toBe(tool.details)
})
it("replaces polling snapshots without duplicate events or retained removed output", () => {
  const first = agentTranscript("chat", turns, [tool])
  const repeated = agentTranscript("chat", turns, [tool])
  expect(repeated.events).toEqual(first.events)
  const cleared = agentTranscript(
    "chat",
    [{ ...turns[0]! }, { ...turns[1]!, text: "", thought: "", parts: [] } as Turn],
    [],
  )
  expect(cleared.turns[0]?.toolCalls).toBe(0)
  expect(cleared.resultByCallId.size).toBe(0)
  expect(cleared.turns[0]?.finalText).toBeNull()
  expect(first.resultByCallId.size).toBe(1)
})
it("retains partial output without claiming completion and excludes waiting messages", () => {
  const transcript = agentTranscript(
    "chat",
    [
      turns[0]!,
      { ...turns[1]!, status: "running" } as Turn,
      { id: "queued", from: "user", receipt: "queued", content: textContent("Later") },
    ],
    [{ ...tool, status: "running" }],
  )
  expect(transcript.turns).toHaveLength(1)
  expect(transcript.turns[0]?.completed).toBeNull()
  expect(transcript.resultByCallId.size).toBe(0)
  expect(transcript.events[2]?.raw).toMatchObject({
    details: tool.details,
    status: "running",
  })
})
it("scopes reused provider tool IDs to their execution and preserves cancellation", () => {
  const second: Turn[] = [
    {
      id: "user2",
      from: "user",
      executionId: "run2",
      content: textContent("Again"),
      receipt: "delivered",
    },
    {
      id: "answer2",
      from: "assistant",
      executionId: "run2",
      text: "",
      status: "cancelled",
      parts: [{ offset: 0, kind: "tool", text: "", toolId: "shell" }],
    },
  ]
  const transcript = agentTranscript(
    "chat",
    [...turns, ...second],
    [tool, { ...tool, executionId: "run2", status: "failed" }],
  )
  expect(transcript.turns).toHaveLength(2)
  expect(transcript.resultByCallId.size).toBe(2)
  expect(transcript.turns[1]?.completed?.payload).toMatchObject({
    status: "interrupted",
    terminalReason: "cancelled",
  })
})

it("places the shared answer after all injected steering inputs without fake failed turns", () => {
  const steered: Turn[] = [
    ...turns,
    {
      id: "steer-one",
      from: "user",
      executionId: "s1",
      steeringTarget: "run",
      steeringOffset: 2,
      content: textContent("hey"),
      receipt: "delivered",
    },
    {
      id: "injected-one",
      from: "assistant",
      executionId: "s1",
      text: "",
      status: "injected",
      parts: [],
    },
    {
      id: "steer-two",
      from: "user",
      executionId: "s2",
      steeringTarget: "run",
      steeringOffset: 2,
      content: textContent("how are you?"),
      receipt: "delivered",
    },
    {
      id: "injected-two",
      from: "assistant",
      executionId: "s2",
      text: "",
      status: "injected",
      parts: [],
    },
  ]
  const result = agentTranscript("chat", steered, [tool])
  expect(result.turns.map((turn) => turn.prompt?.id)).toEqual([
    "user",
    "steer-one",
    "steer-two",
  ])
  expect(result.turns[0]?.toolCalls).toBe(1)
  expect(result.turns[0]?.finalText).toBeNull()
  expect(result.turns[2]?.finalText).toBe("  Done.\n")
  expect(
    result.events.filter((event) => event.payload.type === "turn_completed"),
  ).toHaveLength(1)
  expect(
    result.events.filter((event) => event.payload.type === "user_message"),
  ).toHaveLength(3)
})

it("preserves provider message boundaries, steering positions and exact chunk whitespace", () => {
  const history: Turn[] = [
    turns[0]!,
    {
      id: "response",
      from: "assistant",
      executionId: "run",
      text: "",
      status: "completed",
      parts: [
        { offset: 0, kind: "text", messageId: "before", text: "Before ", toolId: "" },
        { offset: 1, kind: "text", messageId: "before", text: "approval.\n", toolId: "" },
        { offset: 2, kind: "tool", text: "", toolId: "shell" },
        {
          offset: 4,
          kind: "text",
          messageId: "desktop",
          text: "Desktop answer.",
          toolId: "",
        },
        { offset: 5, kind: "text", messageId: "greeting", text: "  I'm ", toolId: "" },
        { offset: 6, kind: "text", messageId: "greeting", text: "well.\n", toolId: "" },
      ],
    },
    {
      id: "steer",
      from: "user",
      executionId: "s",
      steeringTarget: "run",
      steeringOffset: 3,
      receipt: "delivered",
      content: textContent("How are you?"),
    },
  ]
  const result = agentTranscript("chat", history, [tool])
  expect(result.turns.map((turn) => turn.prompt?.id)).toEqual(["user", "steer"])
  expect(result.turns.map((turn) => turn.finalText)).toEqual([
    "Before approval.\n",
    "  I'm well.\n",
  ])
  expect(
    result.events
      .filter((event) => event.payload.type === "assistant_text")
      .map((event) => event.payload),
  ).toEqual([
    {
      type: "assistant_text",
      text: "Before approval.\n",
      block: { messageId: "before", index: 0 },
    },
    {
      type: "assistant_text",
      text: "Desktop answer.",
      block: { messageId: "desktop", index: 4 },
    },
    {
      type: "assistant_text",
      text: "  I'm well.\n",
      block: { messageId: "greeting", index: 5 },
    },
  ])
  expect(agentTranscript("chat", structuredClone(history), [tool]).events).toEqual(
    result.events,
  )
})
