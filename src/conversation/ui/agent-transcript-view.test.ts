import type { JsonValue } from "@nessalabs/agent-stream"
import { expect, it } from "vitest"
import { agentTranscript } from "../adapters/agent-stream/transcript"
import { textContent } from "../model"
import { agentTurnView } from "./agent-transcript-view"

type RawMutation = (raw: Record<string, JsonValue>) => JsonValue

function nullableMetadataTranscript() {
  const transcript = agentTranscript(
    "local",
    [
      {
        id: "local-assistant",
        from: "assistant",
        text: "",
        parts: [
          {
            offset: 0,
            kind: "thought",
            text: "First",
            messageId: "one",
            toolId: "",
            noticeId: "",
          },
          {
            offset: 1,
            kind: "thought",
            text: " second",
            messageId: "two",
            toolId: "",
            noticeId: "",
          },
        ],
      },
    ],
    [],
  )
  const events = transcript.events.filter((event) => event.payload.type === "reasoning")
  expect(events).toHaveLength(2)
  return { transcript, events }
}

function mutateSecondMetadata(mutate: RawMutation) {
  const fixture = nullableMetadataTranscript()
  const second = fixture.events[1]!
  if (second.raw === null || Array.isArray(second.raw) || typeof second.raw !== "object")
    throw new Error("Fixture has no execution metadata")
  Object.defineProperty(second, "raw", { value: mutate(second.raw) })
  return fixture.transcript
}

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
          {
            offset: 0,
            kind: "text",
            text: "Checking.",
            messageId: "a",
            toolId: "",
            noticeId: "",
          },
          { offset: 1, kind: "tool", text: "", toolId: "tool", noticeId: "" },
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
      "text" in part
        ? part.text
        : "notice" in part
          ? part.notice
          : part.activity.map((item) => item.kind),
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
          parts: [{ offset: 0, kind: "tool", text: "", toolId: "tool", noticeId: "" }],
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

it.each([
  ["running", true, false],
  ["completed", false, true],
  ["cancelled", false, true],
  ["failed", false, true],
] as const)(
  "keeps a pre-steering activity tied to its %s execution",
  (status, running, terminal) => {
    const transcript = agentTranscript(
      "chat",
      [
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
          text: "Done",
          status,
          parts: [
            { offset: 0, kind: "tool", text: "", toolId: "tool", noticeId: "" },
            { offset: 2, kind: "text", text: "Done", toolId: "", noticeId: "" },
          ],
        },
        {
          id: "steer",
          from: "user",
          executionId: "steer-run",
          receipt: "delivered",
          content: textContent("Also inspect this"),
          steeringTarget: "run",
          steeringOffset: 1,
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

    expect(transcript.turns).toHaveLength(2)
    expect(transcript.turns[0]?.completed).toBeNull()
    expect(Boolean(transcript.turns[1]?.completed)).toBe(terminal)
    const first = agentTurnView(transcript.turns[0]!, transcript)
    const activity = first.content.find((part) => "activity" in part)
    expect(activity && "activity" in activity ? activity.running : undefined).toBe(
      running,
    )
  },
)

it("settles a thought-only activity from its source execution across steering", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "user",
        from: "user",
        executionId: "run",
        receipt: "delivered",
        content: textContent("Think"),
      },
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "Done",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "Considering", toolId: "", noticeId: "" },
          { offset: 2, kind: "text", text: "Done", toolId: "", noticeId: "" },
        ],
      },
      {
        id: "steer",
        from: "user",
        executionId: "steer-run",
        receipt: "delivered",
        content: textContent("One more thing"),
        steeringTarget: "run",
        steeringOffset: 1,
      },
    ],
    [],
  )

  const first = agentTurnView(transcript.turns[0]!, transcript)
  const activity = first.content.find((part) => "activity" in part)
  expect(activity).toMatchObject({
    activity: [{ kind: "thought", text: "Considering" }],
    running: false,
  })
  expect(transcript.turns[0]?.completed).toBeNull()
  expect(transcript.turns[1]?.completed?.payload.type).toBe("turn_completed")
})

it("uses the source turn identity when a local assistant has no execution id", () => {
  const transcript = agentTranscript(
    "local",
    [
      {
        id: "local-assistant",
        from: "assistant",
        text: "",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "Local thought", toolId: "", noticeId: "" },
        ],
      },
    ],
    [],
  )

  expect(transcript.events[0]?.raw).toMatchObject({
    sourceTurnId: "local-assistant",
    executionId: null,
    executionStatus: "completed",
    activityRunning: false,
  })
  const activity = agentTurnView(transcript.turns[0]!, transcript).content[0]
  expect(activity && "activity" in activity ? activity.running : undefined).toBe(false)
})

it("accepts agreeing source rows with nullable execution identity and status", () => {
  const { transcript } = nullableMetadataTranscript()
  expect(transcript.events[0]?.raw).toMatchObject({
    sourceTurnId: "local-assistant",
    executionId: null,
    executionStatus: null,
    activityRunning: true,
  })
  const activity = agentTurnView(transcript.turns[0]!, transcript).content[0]
  expect(activity).toMatchObject({
    activity: [{ kind: "thought", text: "First second" }],
    running: true,
  })
})

it.each([
  ["source turn", (raw) => ({ ...raw, sourceTurnId: "another-turn" })],
  ["execution id", (raw) => ({ ...raw, executionId: "run" })],
  ["execution status", (raw) => ({ ...raw, executionStatus: "completed" })],
  ["running state", (raw) => ({ ...raw, activityRunning: false })],
] satisfies readonly [string, RawMutation][])(
  "rejects one row that contradicts its source %s",
  (_field, mutate) => {
    const transcript = mutateSecondMetadata(mutate)
    expect(() => agentTurnView(transcript.turns[0]!, transcript)).toThrow(
      "Transcript row combines different execution metadata",
    )
  },
)

it.each([
  ["missing metadata", () => null, "missing"],
  ["source turn", (raw) => ({ ...raw, sourceTurnId: 7 }), "missing"],
  ["execution id", (raw) => ({ ...raw, executionId: false }), "invalid"],
  ["execution status", (raw) => ({ ...raw, executionStatus: [] }), "invalid"],
  ["running state", (raw) => ({ ...raw, activityRunning: "yes" }), "missing"],
] satisfies readonly [string, RawMutation, string][])(
  "rejects a row with %s instead of typed source metadata",
  (_field, mutate, reason) => {
    const transcript = mutateSecondMetadata(mutate)
    expect(() => agentTurnView(transcript.turns[0]!, transcript)).toThrow(reason)
  },
)

it("rejects disagreement between an activity event and its terminal event", () => {
  const transcript = agentTranscript(
    "local",
    [
      {
        id: "local-assistant",
        from: "assistant",
        text: "",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "Finished", toolId: "", noticeId: "" },
        ],
      },
    ],
    [],
  )
  const turn = transcript.turns[0]!
  const terminal = turn.completed
  expect(terminal?.payload.type).toBe("turn_completed")
  if (
    !terminal ||
    terminal.raw === null ||
    Array.isArray(terminal.raw) ||
    typeof terminal.raw !== "object"
  )
    throw new Error("Fixture terminal has no execution metadata")
  Object.defineProperty(terminal, "raw", {
    value: { ...terminal.raw, sourceTurnId: "another-turn" },
  })

  expect(() => agentTurnView(turn, transcript)).toThrow(
    "Transcript row combines different execution metadata",
  )
})

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
          {
            offset: 0,
            kind: "thought",
            text: "hello",
            messageId: "thought",
            toolId: "",
            noticeId: "",
          },
          {
            offset: 1,
            kind: "thought",
            text: " ",
            messageId: "space",
            toolId: "",
            noticeId: "",
          },
          {
            offset: 2,
            kind: "thought",
            text: "world",
            messageId: "world",
            toolId: "",
            noticeId: "",
          },
          { offset: 3, kind: "tool", text: "", toolId: "tool", noticeId: "" },
          {
            offset: 4,
            kind: "thought",
            text: "\n\n",
            messageId: "empty",
            toolId: "",
            noticeId: "",
          },
          {
            offset: 5,
            kind: "text",
            text: "Done",
            messageId: "answer",
            toolId: "",
            noticeId: "",
          },
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
  expect(
    transcript.events
      .filter((event) => event.payload.type === "reasoning")
      .map((event) =>
        event.payload.type === "reasoning" ? event.payload.text : "unreachable",
      ),
  ).toEqual(["hello", " ", "world", "\n\n"])
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
          { offset: 0, kind: "thought", text: "First", toolId: "", noticeId: "" },
          { offset: 1, kind: "tool", text: "", toolId: "one", noticeId: "" },
          { offset: 2, kind: "thought", text: "Second", toolId: "", noticeId: "" },
          { offset: 3, kind: "tool", text: "", toolId: "two", noticeId: "" },
          { offset: 4, kind: "text", text: "Done", toolId: "", noticeId: "" },
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
        parts: [
          { offset: 0, kind: "text", text: "A finder window.", toolId: "", noticeId: "" },
        ],
      },
    ],
    [],
  )
  const rows = transcript.turns.map((turn) => agentTurnView(turn, transcript))
  // `Transcript` looks the user turn up by this id and renders its content.
  expect(rows.map((row) => row.promptId)).toEqual(["pictures"])
})

it("renders distinct Nessa notices in observation order without turning them into permission decisions", () => {
  const transcript = agentTranscript(
    "chat",
    [
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
        text: "Done",
        status: "completed",
        parts: [
          {
            offset: 0,
            kind: "local_notice",
            text: "Nessa declined the first review.",
            toolId: "",
            noticeId: "1",
          },
          {
            offset: 2,
            kind: "local_notice",
            text: "Nessa declined the same review again.",
            toolId: "",
            noticeId: "2",
          },
          { offset: 4, kind: "text", text: "Done", toolId: "", noticeId: "" },
        ],
      },
    ],
    [],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  expect(row.content).toEqual([
    { key: "notice:1", notice: "Nessa declined the first review." },
    { key: "notice:2", notice: "Nessa declined the same review again." },
    { key: "assistant:4", text: "Done" },
  ])
  expect(transcript.events.filter((event) => event.payload.type === "permission_denied")).toEqual(
    [],
  )
})
