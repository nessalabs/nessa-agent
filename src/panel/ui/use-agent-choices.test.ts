// @vitest-environment jsdom
/**
 * A draft's choices are bound to what the gateway's catalog offers now. A
 * host the catalog no longer offers is not kept in the draft: "Run on" shows
 * only offered hosts, so a host nobody can see any more would make every
 * send fail with `environment_not_configured` and no way to change it.
 */
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import type { ConversationSelection } from "../../conversation"
import { useBindCatalogFallback } from "./use-agent-choices"

let container: HTMLElement
let root: Root
const setSelection = vi.fn()

function Draft(props: {
  selection?: ConversationSelection
  serverConversationId?: string
  environments?: readonly string[]
}) {
  useBindCatalogFallback({
    id: "draft",
    agent: "claude",
    model: "sonnet",
    setSelection,
    ...props,
  })
  return null
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  setSelection.mockClear()
  container = document.createElement("div")
  root = createRoot(container)
})

afterEach(() => {
  act(() => root.unmount())
})

const onDevbox: ConversationSelection = {
  agent: "claude",
  model: "sonnet",
  approvalMode: "auto",
  environment: "devbox",
}

it("drops a host the catalog no longer offers from a draft, keeping its other choices", () => {
  act(() =>
    root.render(
      React.createElement(Draft, { selection: onDevbox, environments: ["other"] }),
    ),
  )
  expect(setSelection).toHaveBeenCalledWith("draft", {
    agent: "claude",
    model: "sonnet",
    approvalMode: "auto",
  })
})

it("keeps an offered host, and any host while the catalog is not known", () => {
  act(() =>
    root.render(
      React.createElement(Draft, { selection: onDevbox, environments: ["devbox"] }),
    ),
  )
  act(() => root.render(React.createElement(Draft, { selection: onDevbox })))
  expect(setSelection).not.toHaveBeenCalled()
})

it("leaves a created conversation's selection alone", () => {
  act(() =>
    root.render(
      React.createElement(Draft, {
        selection: onDevbox,
        serverConversationId: "conversation",
        environments: [],
      }),
    ),
  )
  expect(setSelection).not.toHaveBeenCalled()
})

it("binds the catalog's default to a draft with no selection", () => {
  act(() => root.render(React.createElement(Draft, { environments: [] })))
  expect(setSelection).toHaveBeenCalledWith("draft", {
    agent: "claude",
    model: "sonnet",
    approvalMode: "ask",
  })
})
