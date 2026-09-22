import { expect, it } from "vitest"
import { agentTranscript } from "../adapters/agent-stream/transcript"
import { textContent } from "../model"
import { agentTurnView } from "./agent-transcript-view"

it("renders every message in observation order even when the builder extracts finalText", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "user",
        from: "user",
        executionId: "run",
        receipt: "delivered",
        content: textContent("Check"),
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "",
        status: "running",
        parts: [
          { offset: 0, kind: "text", text: "Checking.", messageId: "a", toolId: "" },
          { offset: 1, kind: "tool", text: "", toolId: "tool" },
        ],
      },
      {
        id: "steer",
        from: "user",
        executionId: "s",
        receipt: "delivered",
        content: textContent("Wait"),
        steeringTarget: "run",
        steeringOffset: 2,
      },
    ],
    [
      {
        executionId: "run",
        toolId: "tool",
        title: "Shell",
        input: "{}",
        details: "",
        status: "pending",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  expect(
    row.content.map((part) =>
      "text" in part ? part.text : part.activity.map((item) => item.kind),
    ),
  ).toEqual(["Checking.", ["tool"]])
  const activity = row.content.find((part) => "activity" in part)
  expect(
    activity && "activity" in activity ? activity.activity[0] : undefined,
  ).toMatchObject({
    kind: "tool",
    tool: { title: "Shell", status: "pending" },
  })
})

it.each(["pending", "running", "completed", "failed"])(
  "settles a cancelled execution with a %s tool without overwriting actual results",
  (status) => {
    const transcript = agentTranscript(
      "chat",
      [
        {
          id: "assistant",
          from: "assistant",
          executionId: "run",
          text: "",
          status: "cancelled",
          parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool" }],
        },
      ],
      [
        {
          executionId: "run",
          toolId: "tool",
          title: "Shell",
          input: "{}",
          details: "",
          status,
        },
      ],
    )
    const row = agentTurnView(transcript.turns[0]!, transcript)
    const activity = row.content.find((part) => "activity" in part)
    const item = activity && "activity" in activity ? activity.activity[0] : undefined
    expect(item?.kind === "tool" ? item.tool.status : undefined).toBe(
      status === "completed" || status === "failed" ? status : "stopped",
    )
  },
)

it("keeps streamed whitespace inside a thought and omits a whole whitespace-only run", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "Done",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "hello", messageId: "thought", toolId: "" },
          { offset: 1, kind: "thought", text: " ", messageId: "thought", toolId: "" },
          { offset: 2, kind: "thought", text: "world", messageId: "thought", toolId: "" },
          { offset: 3, kind: "tool", text: "", toolId: "tool" },
          { offset: 4, kind: "thought", text: "\n\n", messageId: "empty", toolId: "" },
          { offset: 5, kind: "text", text: "Done", messageId: "answer", toolId: "" },
        ],
      },
    ],
    [
      {
        executionId: "run",
        toolId: "tool",
        title: "Shell",
        input: "{}",
        details: "done",
        status: "completed",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  const activities = row.content.filter((part) => "activity" in part)
  expect(activities).toHaveLength(1)
  expect(
    activities[0] && "activity" in activities[0] ? activities[0].activity : [],
  ).toEqual([
    { key: "assistant:0", kind: "thought", text: "hello world" },
    {
      key: '["run","tool"]:start',
      kind: "tool",
      tool: expect.objectContaining({ title: "Shell" }),
    },
  ])
})

it("projects alternating thoughts and tools into one ordered turn activity", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "Done",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "First", toolId: "" },
          { offset: 1, kind: "tool", text: "", toolId: "one" },
          { offset: 2, kind: "thought", text: "Second", toolId: "" },
          { offset: 3, kind: "tool", text: "", toolId: "two" },
          { offset: 4, kind: "text", text: "Done", toolId: "" },
        ],
      },
    ],
    [
      {
        executionId: "run",
        toolId: "one",
        title: "One",
        input: "",
        details: "first result",
        status: "failed",
      },
      {
        executionId: "run",
        toolId: "two",
        title: "Two",
        input: "",
        details: "second result",
        status: "completed",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  const activities = row.content.filter((part) => "activity" in part)
  expect(activities).toHaveLength(1)
  expect(
    activities[0] && "activity" in activities[0]
      ? activities[0].activity.map((item) =>
          item.kind === "thought" ? item.text : `${item.tool.title}:${item.tool.status}`,
        )
      : [],
  ).toEqual(["First", "One:failed", "Second", "Two:completed"])
})

it("keeps a row for a turn of images alone, so the transcript has a user turn to paint", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "pictures",
        from: "user",
        executionId: "run",
        receipt: "delivered",
        // No text at all: the row must not depend on there being any.
        content: [
          {
            type: "image-reference",
            digest: `sha256:${"ab".repeat(32)}`,
            mimeType: "image/png",
            size: 2048,
          },
        ],
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "A finder window.",
        status: "completed",
        parts: [{ offset: 0, kind: "text", text: "A finder window.", toolId: "" }],
      },
    ],
    [],
  )
  const rows = transcript.turns.map((turn) => agentTurnView(turn, transcript))
  // `Transcript` looks the user turn up by this id and renders its content.
  expect(rows.map((row) => row.promptId)).toEqual(["pictures"])
})
