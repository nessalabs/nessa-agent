import type { JsonValue } from "@nessalabs/agent-stream"
import { expect, it } from "vitest"
import { agentTranscript } from "../adapters/agent-stream/transcript"
import { textContent } from "../model"
import { agentTurnView } from "./agent-transcript-view"

type RawMutation = (raw: Record<string, JsonValue>) => JsonValue
type Part = ReturnType<typeof agentTurnView>["content"][number]

/** A part's working as a list of kinds, which is how these assertions read it. */
function items(part: Part | undefined) {
  return (part?.work ?? []).map((step) =>
    step.tool
      ? { key: step.key, kind: "tool" as const, tool: step.tool }
      : step.thought !== undefined
        ? { key: step.key, kind: "thought" as const, text: step.thought }
        : { key: step.key, kind: "text" as const, text: step.text ?? "" },
  )
}

function nullableMetadataTranscript() {
  const transcript = agentTranscript(
    "local",
    [
      {
        id: "local-assistant",
        from: "assistant",
        text: "",
        parts: [
          { offset: 0, kind: "thought", text: "First", messageId: "one", toolId: "" },
          { offset: 1, kind: "thought", text: " second", messageId: "two", toolId: "" },
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

it("leaves only the extracted answer outside the turn's one working segment", () => {
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
        kind: "execute",
        input: "{}",
        details: "",
        status: "pending",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  expect(
    row.content.map((part) =>
      "text" in part ? part.text : items(part).map((item) => item.kind),
    ),
    // The builder extracted "Checking." as this running turn's answer so far;
    // everything else the turn did is the one working segment before it.
  ).toEqual([["tool"], "Checking."])
  const activity = row.content.find((part) => "work" in part)
  expect(activity && "work" in activity ? items(activity)[0] : undefined).toMatchObject({
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
          kind: "execute",
          input: "{}",
          details: "",
          status,
        },
      ],
    )
    const row = agentTurnView(transcript.turns[0]!, transcript)
    const activity = row.content.find((part) => "work" in part)
    const item = activity && "work" in activity ? items(activity)[0] : undefined
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
            { offset: 0, kind: "tool", text: "", toolId: "tool" },
            { offset: 2, kind: "text", text: "Done", toolId: "" },
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
          kind: "execute",
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
    const activity = first.content.find((part) => "work" in part)
    expect(activity && "work" in activity ? activity.running : undefined).toBe(running)
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
          { offset: 0, kind: "thought", text: "Considering", toolId: "" },
          { offset: 2, kind: "text", text: "Done", toolId: "" },
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
  const activity = first.content.find((part) => "work" in part)
  expect(activity).toMatchObject({
    work: [{ thought: "Considering" }],
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
        parts: [{ offset: 0, kind: "thought", text: "Local thought", toolId: "" }],
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
  expect(activity && "work" in activity ? activity.running : undefined).toBe(false)
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
    work: [{ thought: "First second" }],
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
        parts: [{ offset: 0, kind: "thought", text: "Finished", toolId: "" }],
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
          { offset: 0, kind: "thought", text: "hello", messageId: "thought", toolId: "" },
          { offset: 1, kind: "thought", text: " ", messageId: "space", toolId: "" },
          { offset: 2, kind: "thought", text: "world", messageId: "world", toolId: "" },
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
        kind: "execute",
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
  const activities = row.content.filter((part) => "work" in part)
  expect(activities).toHaveLength(1)
  expect(activities[0] && "work" in activities[0] ? items(activities[0]) : []).toEqual([
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
        kind: "execute",
        input: "",
        details: "first result",
        status: "failed",
      },
      {
        executionId: "run",
        toolId: "two",
        title: "Two",
        kind: "execute",
        input: "",
        details: "second result",
        status: "completed",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  const activities = row.content.filter((part) => "work" in part)
  expect(activities).toHaveLength(1)
  expect(
    activities[0] && "work" in activities[0]
      ? items(activities[0]).map((item) =>
          item.kind === "tool" ? `${item.tool.title}:${item.tool.status}` : item.text,
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

it("gathers a turn's thinking and tools into one segment, dropping empty thoughts", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "Done.",
        status: "completed",
        parts: [
          { offset: 0, kind: "thought", text: "Looking.", toolId: "" },
          { offset: 1, kind: "tool", text: "", toolId: "one" },
          // Whitespace alone is a disclosure over nothing; it must not add a
          // row, and must not survive as a gap when the adapter coalesces it
          // onto the thought that follows.
          { offset: 2, kind: "thought", text: "\n\n", toolId: "" },
          { offset: 3, kind: "thought", text: "Wrong name.", toolId: "" },
          { offset: 4, kind: "tool", text: "", toolId: "two" },
          { offset: 5, kind: "text", text: "Done.", toolId: "" },
        ],
      },
    ],
    ["one", "two"].map((toolId) => ({
      executionId: "run",
      toolId,
      title: "Shell",
      kind: "execute",
      input: "{}",
      details: "",
      status: "completed",
    })),
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  // One row of working for the whole turn, whatever it took to get there, and
  // in the order it happened so a thought still reads beside its call.
  expect(row.content.map((part) => part.text ?? part.work?.length)).toEqual([4, "Done."])
  expect(row.content[0]?.work?.map((step) => step.thought ?? step.tool?.title)).toEqual([
    "Looking.",
    "Shell",
    "Wrong name.",
    "Shell",
  ])
})

it("keeps a failed tool in the expansion, where somebody can go looking for it", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "I could not read it.",
        status: "completed",
        parts: [
          { offset: 0, kind: "tool", text: "", toolId: "tool" },
          { offset: 1, kind: "text", text: "I could not read it.", toolId: "" },
        ],
      },
    ],
    [
      {
        executionId: "run",
        toolId: "tool",
        title: "Read",
        kind: "execute",
        input: "{}",
        details: "No such file",
        status: "failed",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  const [step] = row.content[0]?.work ?? []
  expect(step?.tool?.status).toBe("failed")
  expect(step?.tool?.details).toBe("No such file")
})

it("keeps what the agent said on the way apart from what it thought", () => {
  const transcript = agentTranscript(
    "chat",
    [
      {
        id: "assistant",
        from: "assistant",
        executionId: "run",
        text: "All set.",
        status: "completed",
        parts: [
          { offset: 0, kind: "text", text: "Let me check.", toolId: "" },
          { offset: 1, kind: "thought", text: "The config moved.", toolId: "" },
          { offset: 2, kind: "text", text: "All set.", toolId: "" },
        ],
      },
    ],
    [],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  expect(row.content[0]?.work).toEqual([
    { key: expect.any(String), text: "Let me check." },
    { key: expect.any(String), thought: "The config moved." },
  ])
  expect(row.content[1]?.text).toBe("All set.")
})

it("keys the answer on the turn, so the bubble is not remounted as the turn goes on", () => {
  const view = (
    text: string,
    parts: { offset: number; kind: "text"; text: string; toolId: string }[],
  ) => {
    const transcript = agentTranscript(
      "chat",
      [
        {
          id: "assistant",
          from: "assistant",
          executionId: "run",
          text,
          status: "running",
          parts,
        },
      ],
      [],
    )
    const row = agentTurnView(transcript.turns[0]!, transcript)
    return row.content.find((part) => part.text !== undefined)?.key
  }
  const said = { offset: 0, kind: "text" as const, text: "Working on it.", toolId: "" }
  // Which text is the answer changes while a turn runs; its key must not.
  expect(view("Working on it.", [said])).toBe(
    view("Here it is.", [said, { ...said, offset: 1, text: "Here it is." }]),
  )
})
