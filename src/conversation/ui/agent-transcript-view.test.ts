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
  expect(row.content.map((part) => part.text ?? part.tools?.[0]?.title)).toEqual([
    "Shell",
    "Checking.",
  ])
  expect(row.tools[0]?.status).toBe("pending")
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
    expect(row.tools[0]?.status).toBe(
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
          // Whitespace alone is a disclosure over nothing; it must not add a row.
          { offset: 2, kind: "thought", text: "\n\n", toolId: "" },
          { offset: 3, kind: "tool", text: "", toolId: "two" },
          { offset: 4, kind: "text", text: "Done.", toolId: "" },
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
  expect(
    row.content.map(
      (part) => part.text ?? `${part.thought ?? ""}/${part.tools?.length ?? 0}`,
    ),
  ).toEqual(["Looking./2", "Done."])
  // One row of working for the whole turn, whatever it took to get there.
  expect(row.content).toHaveLength(2)
})
