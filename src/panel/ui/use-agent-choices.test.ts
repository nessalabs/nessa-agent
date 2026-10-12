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
import type { ConversationChoices } from "../adapters/agent-choices"
import { useBindCatalogFallback, useConversationChoices } from "./use-agent-choices"

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

/** The choices the hook holds, rendered where the test can read them. */
function Choices(props: {
  available: boolean
  follow: (show: (choices: ConversationChoices) => void) => () => void
}) {
  const choices = useConversationChoices(props.available, props.follow)
  return React.createElement("p", null, choices?.catalog.environments.join(",") ?? "none")
}

it("replaces the choices with each catalog the follow sends, and stops it on unmount", () => {
  let show: ((choices: ConversationChoices) => void) | undefined
  const stop = vi.fn()
  const follow = vi.fn((told: (choices: ConversationChoices) => void) => {
    show = told
    return stop
  })
  act(() => root.render(React.createElement(Choices, { available: true, follow })))
  expect(container.textContent).toBe("none")
  act(() =>
    show?.({ catalog: { agents: [], environments: ["devbox"] }, chosenAgent: undefined }),
  )
  expect(container.textContent).toBe("devbox")
  const peer = `peer:${"ab".repeat(32)}`
  act(() =>
    show?.({
      catalog: { agents: [], environments: ["devbox", peer] },
      chosenAgent: undefined,
    }),
  )
  expect(container.textContent).toBe(`devbox,${peer}`)
  expect(follow).toHaveBeenCalledOnce()
  expect(stop).not.toHaveBeenCalled()
  act(() => root.unmount())
  expect(stop).toHaveBeenCalledOnce()
  // `afterEach` unmounts again; a fresh root keeps that harmless.
  root = createRoot(container)
})

it("stops the follow and offers nothing while the gateway is unavailable", () => {
  let show: ((choices: ConversationChoices) => void) | undefined
  const stop = vi.fn()
  const follow = (told: (choices: ConversationChoices) => void) => {
    show = told
    return stop
  }
  act(() => root.render(React.createElement(Choices, { available: true, follow })))
  act(() =>
    show?.({ catalog: { agents: [], environments: ["devbox"] }, chosenAgent: undefined }),
  )
  act(() => root.render(React.createElement(Choices, { available: false, follow })))
  expect(stop).toHaveBeenCalledOnce()
  expect(container.textContent).toBe("none")
})
