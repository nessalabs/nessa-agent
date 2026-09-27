// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { conversation } from "../model"
import { ConversationDetails } from "./conversation-details"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

function attached(
  overrides: Partial<NonNullable<ReturnType<typeof conversation>["remote"]>> = {},
) {
  const item = conversation("conversation")
  item.remote = {
    questions: [],
    running: false,
    permissions: [],
    tools: [],
    pending: [],
    queueComplete: true,
    truncated: false,
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported_not_implemented",
        policyCloseSession: "unsupported_not_implemented",
        incomingElicitation: "supported_with_correlated_round_trip",
      },
    },
    lifecycle: { phase: "attached" },
    ...overrides,
  }
  return item
}

function show(item: ReturnType<typeof conversation>) {
  act(() => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
      />,
    )
  })
}

function group(title: string) {
  const section = document.body.querySelector(`section[aria-label="${title}"]`)
  return section?.textContent ?? null
}

it("says in a word whether approvals work here, without the capability jargon", () => {
  show(attached())

  expect(group("Approvals")).toBe(
    "ApprovalsDeny a requestSupportedAnswer laterNot yet in Nessa",
  )
  expect(group("Model")).toBe("ModelImagesNot supported")
  expect(document.body.textContent).not.toContain("turn correlation")
})

it("names the model and its agent once the gateway reports them", () => {
  show(
    attached({
      runtime: {
        model: "claude-sonnet-5",
        provider: "claude-acp",
        workspace: "/work/nessa",
        agent: "claude",
        modelName: "Sonnet 5",
        contextWindowTokens: 1_000_000,
        reasoning: true,
      },
      approvalMode: "auto",
    }),
  )

  expect(document.body.textContent).toContain("Sonnet 5Claude")
  expect(document.body.querySelector("p > svg")).not.toBeNull()
  expect(group("Approvals")).toContain("Tool callsAutomatic")
  expect(group("Workspace")).toBe("Workspace/work/nessa")
  expect(group("Model")).toBe(
    "ModelContext window1 million tokensReasoningOnImagesNot supported",
  )
})

it("falls back to the identifier and harness when the gateway sends no agent", () => {
  show(
    attached({
      runtime: { model: "gpt-5.6-terra", provider: "codex-acp", workspace: "/w" },
    }),
  )

  expect(document.body.textContent).toContain("gpt-5.6-terracodex-acp")
  expect(group("Approvals")).not.toContain("Tool calls")
  expect(group("Model")).toBe("ModelImagesNot supported")
})

it("writes a context window that is not a round million as a plain count", () => {
  show(
    attached({
      runtime: {
        model: "m",
        provider: "p",
        workspace: "/w",
        contextWindowTokens: 1_050_000,
        reasoning: false,
      },
    }),
  )

  expect(group("Model")).toBe(
    "ModelContext window1,050,000 tokensReasoningOffImagesNot supported",
  )
})

it("names no agent for an id setup does not list", () => {
  show(
    attached({
      runtime: {
        model: "m",
        provider: "p",
        workspace: "/w",
        agent: "constructor",
      },
    }),
  )

  expect(document.body.querySelector("p > svg")).toBeNull()
  expect(document.body.textContent).toContain("mp")
})
