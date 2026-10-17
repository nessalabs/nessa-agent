// @vitest-environment jsdom
/**
 * The floating card's countdown reads the injected clock again each second.
 * A single `Date.now()` at render leaves the sentence where it started.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { act } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { permissionTargetConversation } from "@nessa/client"

import type { AgentFeatures, Conversation } from "../model"
import { ClockProvider } from "../../clock"

const dispatch = vi.fn()
vi.mock("../adapters/store/hooks", () => ({ useConversationDispatch: () => dispatch }))
vi.mock("../adapters/store/slice", () => ({
  controlConversation: (request: unknown) => ({ type: "control", request }),
  refollowConversation: (id: string) => ({ type: "refollow", id }),
}))

const { ConversationControls } = await import("./conversation-controls")

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const agentFeatures: AgentFeatures = {
  permissionDenial: "unknown",
  nativeHookSuppression: "unknown",
  compactionReporting: "unsupported_not_implemented",
  modelSwitchReporting: "unsupported_not_implemented",
  permissionDeferral: "unsupported_not_implemented",
  elicitationForwarding: "unknown",
  preToolPolicy: "unsupported_not_implemented",
  policyEndTurn: "unsupported",
  policyCloseSession: "unsupported",
  incomingElicitation: "unsupported",
  nativeHooks: "off",
}

function waiting(dueAtMs: number): Conversation {
  return {
    id: "conversation",
    title: "Countdown",
    phase: "idle",
    draft: [],
    turns: [],
    remote: {
      approvalMode: "ask",
      approvalModes: [{ id: "ask", name: "Ask", description: "Ask first." }],
      artifacts: [],
      notes: [],
      questions: [],
      running: true,
      reviewsWaiting: [],
      questionsWaiting: [],
      permissions: [
        {
          review: "0".repeat(32),
          executionId: "run",
          permissionId: "permission",
          toolId: "tool",
          title: "Run tests",
          toolName: "Shell",
          argumentsJson: "{}",
          options: [{ id: "cancel", label: "Cancel" }],
          deadline: {
            waitMs: 30_000,
            dueAtMs,
            effect: "deny",
            reason: "No human answered in time.",
            stopsTurn: false,
          },
        },
      ],
      tools: [],
      pending: [],
      capabilities: {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        imageInput: false,
        agentFeatures,
      },
      lifecycle: { phase: "attached" },
      queueComplete: true,
      transcriptState: "complete",
      truncated: false,
    },
  }
}

let host: HTMLDivElement
let root: Root
let time = 1_700_000_000_000

beforeEach(() => {
  dispatch.mockClear()
  vi.useFakeTimers()
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })
  Element.prototype.animate = vi.fn(() => ({ cancel: vi.fn() })) as never
  time = 1_700_000_000_000
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
  vi.useRealTimers()
})

it("says when the published view left history out or has not confirmed it", async () => {
  const base = waiting(time + 30_000)
  const conversation: Conversation = {
    ...base,
    remote: base.remote && {
      ...base.remote,
      permissions: [],
      truncated: true,
      transcriptState: "stale",
    },
  }
  await act(async () => {
    root.render(
      <ClockProvider now={() => time}>
        <ConversationControls conversation={conversation} gatewayAvailable />
      </ClockProvider>,
    )
  })
  const said = [...host.querySelectorAll("[role=status]")].map(
    (notice) => notice.textContent,
  )
  expect(said).toEqual([
    "Some conversation content is not shown.",
    "Conversation history is not confirmed. Recent activity may be missing.",
  ])
})

describe("the floating card countdown", () => {
  it("reads the injected clock again each second", async () => {
    const dueAtMs = time + 30_000
    await act(async () => {
      root.render(
        <ClockProvider now={() => time}>
          <ConversationControls conversation={waiting(dueAtMs)} gatewayAvailable />
        </ClockProvider>,
      )
    })
    expect(host.textContent).toContain(
      "If nobody answers within 30 seconds, this is denied automatically.",
    )
    await act(async () => {
      time += 5_000
      await vi.advanceTimersByTimeAsync(1_000)
    })
    expect(host.textContent).toContain(
      "If nobody answers within 25 seconds, this is denied automatically.",
    )
  })
})

it("captures the displayed review's conversation for both answer and dismiss", async () => {
  const conversation = { ...waiting(time + 30_000), serverConversationId: "captured" }
  await act(async () => {
    root.render(
      <ClockProvider now={() => time}>
        <ConversationControls conversation={conversation} gatewayAvailable />
      </ClockProvider>,
    )
  })
  const buttons = [...host.querySelectorAll("button")]
  expect(buttons.map((button) => button.textContent)).toEqual([
    "Cancel",
    "Dismiss request",
  ])
  await act(async () => {
    buttons.forEach((button) => button.click())
  })
  const controls = dispatch.mock.calls.map(([action]) => action.request.control)
  expect(controls.map((control) => control.kind)).toEqual(["answer", "cancel"])
  for (const control of controls) {
    expect(permissionTargetConversation(control.target)).toBe("captured")
    expect(control.target).toMatchObject({
      review: "0".repeat(32),
      executionId: "run",
      permissionId: "permission",
    })
    expect(control).not.toHaveProperty("executionId")
    expect(control).not.toHaveProperty("permissionId")
  }
})

it("offers no answer on a review until it has loaded whole, and offers trying again when it could not", async () => {
  const base = waiting(time + 30_000)
  const remote = base.remote
  const [review] = remote?.permissions ?? []
  if (!remote || !review) throw new Error("the sample asks for nothing")
  const key = "0123456789abcdef0123456789abcdef"
  const listed = (failed: boolean | null): Conversation => ({
    ...base,
    remote: {
      ...remote,
      reviewsWaiting:
        failed === null ? [] : [{ review: key, toolName: review.toolName, failed }],
      // A 100 KB Write, once loaded.
      permissions:
        failed === null
          ? [{ ...review, argumentsJson: `{"content":"${"x".repeat(99_986)}"}` }]
          : [],
    },
  })
  const show = (conversation: Conversation) =>
    act(async () => {
      root.render(
        <ClockProvider now={() => time}>
          <ConversationControls conversation={conversation} gatewayAvailable />
        </ClockProvider>,
      )
    })
  const button = (label: string) =>
    [...host.querySelectorAll("button")].find((each) => each.textContent === label)
  await show(listed(false))
  expect(host.textContent).toContain("Loading this review before you can answer")
  expect(button("Cancel")).toBeUndefined()
  expect(button("Try again")).toBeUndefined()
  await show(listed(true))
  expect(host.textContent).toContain("This review could not be loaded")
  expect(button("Cancel")).toBeUndefined()
  await act(async () => button("Try again")?.click())
  expect(dispatch).toHaveBeenCalledWith({ type: "refollow", id: base.id })
  await show(listed(null))
  expect(host.textContent).not.toContain("this review")
  expect(button("Cancel")?.disabled).toBe(false)
})
