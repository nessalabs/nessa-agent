import { describe, expect, it } from "vitest"
import {
  emptyTranscript,
  groupSteps,
  inlineRuns,
  messageText,
  sameWords,
  titleFrom,
  titleLength,
  toolWidget,
  unconfirmed,
  type Message,
  type Part,
} from "./transcript"
import { sameWidget } from "./widget-ref"

describe("an agent message's parts", () => {
  it("gathers consecutive steps into one group, keeping everything else in order", () => {
    const parts: Part[] = [
      { kind: "step", step: "read", label: "Read", detail: "a.ts" },
      { kind: "step", step: "edit", label: "Edited", detail: "a.ts", added: 3 },
      { kind: "text", text: "Done." },
      { kind: "step", step: "run", label: "Ran", detail: "cargo test" },
    ]
    expect(groupSteps(parts)).toEqual([[parts[0], parts[1]], parts[2], [parts[3]]])
    expect(groupSteps([])).toEqual([])
  })

  it("marks out inline code and strong text, leaving the rest plain", () => {
    expect(inlineRuns("Use `SplitView` for **every** pane.")).toEqual([
      { kind: "text", text: "Use " },
      { kind: "code", text: "SplitView" },
      { kind: "text", text: " for " },
      { kind: "strong", text: "every" },
      { kind: "text", text: " pane." },
    ])
  })

  it("keeps unmatched or empty marks as text", () => {
    expect(inlineRuns("a ` b ** c")).toEqual([{ kind: "text", text: "a ` b ** c" }])
    expect(inlineRuns("``")).toEqual([{ kind: "text", text: "``" }])
    expect(inlineRuns("")).toEqual([])
  })

  it("reads a message's prose and list items as its text", () => {
    expect(
      messageText({
        id: "m",
        role: "agent",
        at: 0,
        parts: [
          { kind: "step", step: "read", label: "Read" },
          { kind: "text", text: "Here:" },
          { kind: "list", items: ["one", "two"] },
          { kind: "code", code: "let x = 1" },
        ],
      }),
    ).toBe("Here: one two")
  })

  it("passes over a widget when reading a message's text, and ends a group of steps at one", () => {
    const widget: Part = { kind: "widget", widget: { plugin: "mcp:charts", id: "call" } }
    const parts: Part[] = [
      { kind: "step", step: "run", label: "Ran", detail: "chart" },
      widget,
      { kind: "step", step: "read", label: "Read" },
      { kind: "text", text: "Here it is." },
    ]
    expect(groupSteps(parts)).toEqual([[parts[0]], widget, [parts[2]], parts[3]])
    expect(messageText({ id: "m", role: "agent", at: 0, parts })).toBe("Here it is.")
  })
})

describe("a tool call's widget", () => {
  const call = { executionId: "run", toolId: "call-1" }
  const ui = { server: "charts", tool: "show", resourceUri: "ui://charts/view.html" }

  it("is the MCP server's app drawing that call, when the tool declared a UI", () => {
    expect(toolWidget({ ...call, mcp: ui })).toEqual({
      kind: "widget",
      widget: { plugin: "mcp:charts", id: JSON.stringify(["run", "call-1"]) },
    })
  })

  it("is none for a tool without UI, an MCP tool without one, or a harness that did not say", () => {
    expect(toolWidget(call)).toBeNull()
    expect(toolWidget({ ...call, mcp: { server: "charts", tool: "show" } })).toBeNull()
    expect(toolWidget({ ...call, mcp: { ...ui, resourceUri: "" } })).toBeNull()
  })

  it("names each call apart, however its identities are spelt", () => {
    const one = toolWidget({ executionId: "a:b", toolId: "c", mcp: ui })!
    const other = toolWidget({ executionId: "a", toolId: "b:c", mcp: ui })!
    expect(sameWidget(one.widget, other.widget)).toBe(false)
    expect(
      sameWidget(
        one.widget,
        toolWidget({ executionId: "a:b", toolId: "c", mcp: ui })!.widget,
      ),
    ).toBe(true)
  })
})

describe("titles and previews", () => {
  it("titles a session by its first sentence, capitalised, without end punctuation", () => {
    expect(titleFrom("fix the rain. Then the steam.")).toBe("Fix the rain")
    expect(titleFrom("why does it flicker?\nIt's bad")).toBe("Why does it flicker")
  })

  it("cuts a long first sentence at a word", () => {
    const title = titleFrom(
      "Let the workspace hold two or three conversations side by side without squeezing",
    )
    expect(title.endsWith("…")).toBe(true)
    expect(title.length).toBeLessThanOrEqual(titleLength + 1)
    expect(title).toBe("Let the workspace hold two or three…")
  })

  it("tells when a preview only repeats the title", () => {
    expect(sameWords("Fix the rain", "fix  the rain.")).toBe(true)
    expect(sameWords("Fix the rain", "Fix the steam")).toBe(false)
  })
})

describe("messages the person sent", () => {
  const sent = (id: string): Message => ({
    id,
    role: "user",
    at: 0,
    parts: [{ kind: "text", text: id }],
    delivery: { state: "sending" },
  })

  it("stay until the source's conversation holds them by id", () => {
    const outbox = [sent("m1"), sent("m2")]
    expect(unconfirmed(outbox, undefined)).toBe(outbox)
    expect(unconfirmed(outbox, emptyTranscript("s"))).toBe(outbox)
    const held = {
      ...emptyTranscript("s"),
      messages: [{ ...sent("m1"), delivery: undefined }],
    }
    expect(unconfirmed(outbox, held).map((message) => message.id)).toEqual(["m2"])
  })
})
