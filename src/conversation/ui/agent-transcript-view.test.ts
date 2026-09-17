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
  expect(row.content.map((part) => part.text ?? part.tools?.[0]?.title)).toEqual([
    "Checking.",
    "Shell",
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
