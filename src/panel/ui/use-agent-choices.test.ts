// @vitest-environment jsdom
/**
 * A draft with no choice yet is bound to what the gateway's catalog offers
 * now; a choice it already holds, or a created conversation's, is left alone.
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
const OFF = { claude: "off", codex: "off" } as const

const setSelection = vi.fn()

function Draft(props: {
  selection?: ConversationSelection
  serverConversationId?: string
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

const chosen: ConversationSelection = {
  agent: "claude",
  model: "sonnet",
  approvalMode: "auto",
}

it("keeps a draft's own choice", () => {
  act(() => root.render(React.createElement(Draft, { selection: chosen })))
  expect(setSelection).not.toHaveBeenCalled()
})

it("leaves a created conversation alone", () => {
  act(() =>
    root.render(React.createElement(Draft, { serverConversationId: "conversation" })),
  )
  expect(setSelection).not.toHaveBeenCalled()
})

it("binds the catalog's default to a draft with no selection", () => {
  act(() => root.render(React.createElement(Draft, {})))
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
  return React.createElement("p", null, choices?.catalog.nativeHooks.claude ?? "none")
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
    show?.({
      catalog: { agents: [], nativeHooks: OFF },
      chosenAgent: undefined,
    }),
  )
  expect(container.textContent).toBe("off")
  act(() =>
    show?.({
      catalog: { agents: [], nativeHooks: { ...OFF, claude: "on" } },
      chosenAgent: undefined,
    }),
  )
  expect(container.textContent).toBe("on")
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
    show?.({
      catalog: { agents: [], nativeHooks: OFF },
      chosenAgent: undefined,
    }),
  )
  act(() => root.render(React.createElement(Choices, { available: false, follow })))
  expect(stop).toHaveBeenCalledOnce()
  expect(container.textContent).toBe("none")
})
