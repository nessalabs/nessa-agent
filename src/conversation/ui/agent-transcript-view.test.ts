import { expect, it } from "vitest"
import { agentTranscript } from "../adapters/agent-stream/transcript"
import { textContent } from "../model"
import { agentTurnView } from "./agent-transcript-view"

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
        input: "{}",
        details: "",
        status: "pending",
      },
    ],
  )
  const row = agentTurnView(transcript.turns[0]!, transcript)
  // The turn's working is one segment; the answer the builder extracted is
  // the only thing left outside it.
  expect(
    row.content.map((part) => part.text ?? part.work?.map((step) => step.tool?.title)),
  ).toEqual([["Shell"], "Checking."])
  expect(row.content[0]?.work?.[0]?.tool?.status).toBe("pending")
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
    expect(row.content[0]?.work?.[0]?.tool?.status).toBe(
      status === "completed" || status === "failed" ? status : "stopped",
    )
  },
)

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
