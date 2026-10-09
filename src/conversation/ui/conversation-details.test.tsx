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
    approvalMode: "ask",
    approvalModes: [
      { id: "ask", name: "Provider asks", description: "The provider asks." },
    ],
    questions: [],
    running: false,
    permissions: [],
    tools: [],
    pending: [],
    queueComplete: true,
    transcriptState: "complete",
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
  const section = [...document.body.querySelectorAll("section")].find(
    (item) =>
      document.getElementById(item.getAttribute("aria-labelledby") ?? "")?.textContent ===
      title,
  )
  return section?.textContent ?? null
}

it("names each group by an h3 under the sheet's h2", () => {
  show(attached())
  const headings = [...document.body.querySelectorAll("section > h3")].map(
    (heading) => heading.textContent,
  )
  expect(headings).toEqual(["Model", "Approvals"])
  expect(document.body.querySelector("section h4")).toBeNull()
})

it("says in a word whether approvals work here, without the capability jargon", () => {
  show(attached())

  expect(group("Approvals")).toBe(
    "ApprovalsTool callsProvider asksDeny a requestSupportedAnswer laterNot yet in Nessa",
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
      approvalModes: [
        { id: "ask", name: "Provider asks", description: "The provider asks." },
        { id: "auto", name: "Automatic review", description: "The provider reviews." },
      ],
    }),
  )

  expect(document.body.textContent).toContain("Sonnet 5Claude")
  expect(document.body.querySelector("p > svg")).not.toBeNull()
  expect(group("Approvals")).toContain("Tool callsAutomatic review")
  expect(group("Workspace")).toBe("Workspace/work/nessa")
  expect(group("Model")).toBe(
    "ModelModel max context1 million tokensReasoningOnImagesNot supported",
  )
})

it("writes a context window that is not a round million as a plain count", () => {
  show(
    attached({
      runtime: {
        model: "m",
        modelName: "Model M",
        agent: "codex",
        provider: "p",
        workspace: "/w",
        contextWindowTokens: 1_050_000,
        reasoning: false,
      },
    }),
  )

  expect(group("Model")).toBe(
    "ModelModel max context1,050,000 tokensReasoningOffImagesNot supported",
  )
})

it("names no agent for an id setup does not list", () => {
  show(
    attached({
      runtime: {
        model: "m",
        modelName: "Model M",
        provider: "p",
        workspace: "/w",
        agent: "constructor",
        contextWindowTokens: 100_000,
        reasoning: false,
      },
    }),
  )

  expect(document.body.querySelector("p > svg")).toBeNull()
  expect(document.body.textContent).toContain("Model Mp")
})

it("says where the agent runs and the sandbox around it, from its lease", () => {
  show(
    attached({
      lease: {
        state: "live",
        revision: 1,
        environment: "here",
        sandbox: "harness_default",
        droppedEvents: 0,
      },
    }),
  )

  expect(group("Where it runs")).toBe(
    "Where it runsComputerThis computerSandboxThe agent's ownStatusRunning",
  )
})

it("says why a lease ended, or why none was granted", () => {
  type Lease = NonNullable<NonNullable<ReturnType<typeof attached>["remote"]>["lease"]>
  const cases: [Lease, string][] = [
    [{ state: "ending", cause: "closed", droppedEvents: 0 }, "Stopping"],
    [{ state: "ended", cause: "closed", droppedEvents: 0 }, "Closed"],
    [{ state: "ended", cause: "stopped", droppedEvents: 0 }, "Stopped"],
    [{ state: "ended", cause: "revoked", droppedEvents: 0 }, "Access withdrawn"],
    [{ state: "ended", cause: "expired", droppedEvents: 0 }, "Timed out"],
    [{ state: "ended", cause: "lost", droppedEvents: 0 }, "Ended when Nessa restarted"],
    [
      { state: "interrupted", cause: "stopped", droppedEvents: 0 },
      "Cleanup not confirmed",
    ],
    [
      { state: "refused", refusal: "sandbox_unavailable", droppedEvents: 0 },
      "Couldn't start: sandbox not available",
    ],
    [{ state: "refused", droppedEvents: 0 }, "Couldn't start"],
    [{ state: "unreadable", droppedEvents: 0 }, "Not known"],
  ]
  for (const [lease, status] of cases) {
    show(attached({ lease }))
    expect(group("Where it runs")).toBe(
      `Where it runsComputerNot knownSandboxNot knownStatus${status}`,
    )
  }
})
