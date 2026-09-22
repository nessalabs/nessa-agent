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

it("describes scoped support without implying missing policy integrations", () => {
  const item = conversation("conversation")
  item.remote = {
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
        incomingElicitation: "unsupported_not_implemented",
      },
    },
  }

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

  expect(document.body.textContent).toContain(
    "Deny requested tool accessAvailable when the agent offers a deny choice",
  )
  expect(document.body.textContent).toContain("Isolate user hooksNot verified")
  expect(document.body.textContent).toContain(
    "Explicitly defer permission decisionsExplicit later-answer outcomes are not implemented in Nessa",
  )
  expect(document.body.textContent).toContain(
    "Apply policies before tools runNot implemented in Nessa",
  )
  expect(document.body.textContent).toContain(
    "End a turn from policyNot implemented in Nessa",
  )
  expect(document.body.textContent).toContain(
    "Close a session from policyNot implemented in Nessa",
  )
  expect(document.body.textContent).toContain(
    "Answer incoming questions in NessaNot implemented in Nessa",
  )
})
