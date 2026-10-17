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
    artifacts: [],
    notes: [],
    approvalMode: "ask",
    approvalModes: [
      { id: "ask", name: "Provider asks", description: "The provider asks." },
    ],
    questions: [],
    running: false,
    reviewsWaiting: [],
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
        nativeHooks: "off",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unsupported_not_implemented",
        policyEndTurn: "unsupported",
        policyCloseSession: "unsupported",
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

/** A conversation whose agent process has started, as the gateway reports it. */
function started() {
  return attached({
    runtime: {
      model: "claude-sonnet-5",
      provider: "claude-acp",
      workspace: "/work/nessa",
      agent: "claude",
      modelName: "Sonnet 5",
      contextWindowTokens: 200_000,
      reasoning: true,
    },
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

it("claims no run's hooks before an agent process has started", () => {
  // A conversation followed after a restart has no process, and its default
  // capabilities read off whatever its last run used.
  show(attached())
  expect(group("Approvals")).not.toContain("Native hooks")

  show(started())
  expect(group("Approvals")).toContain(
    "Native hooksFor this run, Nessa asks this agent to leave its own hooks off.",
  )
})

it("says a hook can change a tool call when native hooks are on", () => {
  const item = attached()
  const features = item.remote?.capabilities.agentFeatures
  if (!features) throw new Error("missing features")
  features.nativeHooks = "on"
  if (!item.remote) throw new Error("missing remote")
  item.remote.runtime = {
    model: "claude-sonnet-5",
    provider: "claude-acp",
    workspace: "/work/nessa",
    agent: "claude",
    modelName: "Sonnet 5",
    contextWindowTokens: 200_000,
    reasoning: true,
  }
  show(item)
  expect(group("Approvals")).toContain(
    "For this run, hooks can approve, deny, or change a tool call and its result without Nessa asking, and can run commands when the agent starts.",
  )
  expect(group("Approvals")).toContain("only for projects you trust")
  expect(group("Approvals")).not.toContain("The next time this agent starts")
})

it("says the current switch applies at the next start when this run differs", async () => {
  const item = attached()
  const features = item.remote?.capabilities.agentFeatures
  if (!features) throw new Error("missing features")
  features.nativeHooks = "on"
  if (!item.remote) throw new Error("missing remote")
  item.remote.runtime = {
    model: "claude-sonnet-5",
    provider: "claude-acp",
    workspace: "/work/nessa",
    agent: "claude",
    modelName: "Sonnet 5",
    contextWindowTokens: 200_000,
    reasoning: true,
  }
  await act(async () => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
        readNativeHooks={async () => "off"}
      />,
    )
  })
  expect(group("Approvals")).toContain(
    "The next time this agent starts, Nessa asks it to leave its own hooks off.",
  )
  expect(group("Approvals")).not.toContain("so they can run")
})

it("names hooks on, and warns about the repo's settings, when Claude's next start turns them on", async () => {
  const item = attached()
  if (!item.remote) throw new Error("missing remote")
  item.remote.runtime = {
    model: "claude-sonnet-5",
    provider: "claude-acp",
    workspace: "/work/nessa",
    agent: "claude",
    modelName: "Sonnet 5",
    contextWindowTokens: 200_000,
    reasoning: true,
  }
  await act(async () => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
        readNativeHooks={async () => "on"}
      />,
    )
  })
  expect(group("Approvals")).toContain(
    "For this run, Nessa asks this agent to leave its own hooks off. The next time this agent starts, Nessa no longer asks it to leave its own hooks off, so they can run.",
  )
  expect(group("Approvals")).toContain("only for projects you trust")
})

it("reads the switch again when the window is focused", async () => {
  const item = started()
  let choice: "off" | "on" = "on"
  await act(async () => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
        readNativeHooks={async () => choice}
      />,
    )
  })
  expect(group("Approvals")).toContain(
    "The next time this agent starts, Nessa no longer asks it to leave its own hooks off, so they can run.",
  )
  choice = "off"
  await act(async () => {
    window.dispatchEvent(new Event("focus"))
  })
  expect(group("Approvals")).toContain(
    "For this run, Nessa asks this agent to leave its own hooks off.",
  )
  expect(group("Approvals")).not.toContain("The next time this agent starts")
})

it("keeps the newest read when an older one settles last", async () => {
  const item = started()
  const pending: Array<(choice: "off" | "on") => void> = []
  await act(async () => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
        readNativeHooks={() => new Promise((resolve) => pending.push(resolve))}
      />,
    )
  })
  await act(async () => pending[0]("off"))
  await act(async () => {
    window.dispatchEvent(new Event("focus"))
    window.dispatchEvent(new Event("focus"))
  })
  // The newer read answers first, then the older one with an outdated switch.
  await act(async () => pending[2]("off"))
  await act(async () => pending[1]("on"))
  expect(group("Approvals")).toContain(
    "For this run, Nessa asks this agent to leave its own hooks off.",
  )
  expect(group("Approvals")).not.toContain("The next time this agent starts")
})

it("keeps this run's sentence when the switch still matches", async () => {
  const item = started()
  await act(async () => {
    root.render(
      <ConversationDetails
        conversation={item}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
        readNativeHooks={async () => "off"}
      />,
    )
  })
  expect(group("Approvals")).toContain(
    "For this run, Nessa asks this agent to leave its own hooks off.",
  )
  expect(group("Approvals")).not.toContain("The next time this agent starts")
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
    "Where it runsComputerThis computerSandboxThe agent's ownStatusAllowed to run",
  )

  show(
    attached({
      lease: {
        state: "live",
        revision: 1,
        environment: "here",
        sandbox: "none",
        droppedEvents: 0,
      },
    }),
  )
  expect(group("Where it runs")).toBe(
    "Where it runsComputerThis computerSandboxNoneStatusAllowed to run",
  )
})

it("says why a lease ended, and of a refused one only why nothing runs", () => {
  show(
    attached({
      lease: {
        state: "ended",
        environment: "here",
        sandbox: "harness_default",
        cause: "lost",
        droppedEvents: 0,
      },
    }),
  )
  expect(group("Where it runs")).toBe(
    "Where it runsComputerThis computerSandboxThe agent's ownStatusEnded when Nessa restarted",
  )

  // The sandbox a refused lease carries is the one it asked for.
  show(
    attached({
      lease: {
        state: "refused",
        revision: 1,
        environment: "here",
        sandbox: "harness_default",
        refusal: "sandbox_unavailable",
        droppedEvents: 0,
      },
    }),
  )
  expect(group("Where it runs")).toBe(
    "Where it runsStatusCouldn't start: sandbox not available",
  )

  show(attached({ lease: { state: "unreadable", droppedEvents: 0 } }))
  expect(group("Where it runs")).toBe(
    "Where it runsComputerNot knownSandboxNot knownStatusNot known",
  )
})
