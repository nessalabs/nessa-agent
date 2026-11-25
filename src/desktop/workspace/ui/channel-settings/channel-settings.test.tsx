// @vitest-environment jsdom
/**
 * A made channel's coordinator, switched in its settings: a switch in flight
 * holds the row, and a refusal says why. Every agent runs on this gateway's
 * own machine (ADR 1417), so there is no place to choose.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import { createQuestionFlights } from "../../application/question-flights"
import { ChannelCreationError, type ChannelSource } from "../../application/ports"
import type { Channel } from "../../model/workspace-index"
import { loadWorkspace } from "../../adapters/store/commands"
import { fakeSource, keptFilter, roomyGrid, settle, testIndex } from "../../testing"
import { ChannelCoordinatorControl } from "./channel-settings"

let root: Root
let host: HTMLDivElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

let index = testIndex()
let channel = index.channels[0] as Channel

const openMenus = () => document.querySelectorAll('[role="menu"]').length

async function closeMenus() {
  if (openMenus() === 0) return
  await act(async () => {
    ;(document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    )
  })
}

/** The coordinator switch over a store whose made channel is on. */
async function switchShown(
  setCoordinator: (id: string, on: boolean) => Promise<Channel>,
) {
  index = testIndex()
  const found = index.channels.find((each) => each.id !== "default") as Channel
  channel = { ...found, coordinatorOn: true, coordinatorConversationId: "main" }
  const listed = index.channels as Channel[]
  listed[listed.indexOf(found)] = channel
  let next = 0
  const store = makeDesktopStore({
    workspace: fakeSource(index),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    channels: {
      create: () => Promise.reject(new Error("unused")),
      setCoordinator,
    } as ChannelSource,
  })
  await store.dispatch(loadWorkspace())
  await settle()
  await act(async () => {
    root.render(
      <Provider store={store}>
        <ChannelCoordinatorControl channelId={channel.id} />
      </Provider>,
    )
  })
  await act(settle)
}

async function chooseCoordinator(label: "On" | "Off") {
  await closeMenus()
  const button = host.querySelector("[data-channel-coordinator] [data-choice-trigger]")
  if (!(button instanceof HTMLButtonElement)) throw new Error("no coordinator menu")
  await act(async () => {
    button.focus()
    button.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    )
  })
  const found = [
    ...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
  ].find((each) => each.textContent === label)
  if (!found) throw new Error(`no ${label}`)
  await act(async () => found.click())
  await act(settle)
}

it("switches the coordinator off and on, keeping the same main chat", async () => {
  const asked: boolean[] = []
  await switchShown(async (_id, on) => {
    asked.push(on)
    // The source lists it switched too, as the gateway's list does.
    const switched = { ...channel, coordinatorOn: on }
    const listed = index.channels as Channel[]
    listed[listed.findIndex((each) => each.id === channel.id)] = switched
    return switched
  })
  const group = () => host.querySelector("[data-channel-coordinator]")
  expect(group()?.getAttribute("data-coordinator-on")).toBe("true")
  await chooseCoordinator("Off")
  expect(asked).toEqual([false])
  expect(group()?.getAttribute("data-coordinator-on")).toBe("false")
  expect(group()?.textContent).toContain("Its chat and threads stay.")
  // Choosing what it already is asks nothing.
  await chooseCoordinator("Off")
  expect(asked).toEqual([false])
  await chooseCoordinator("On")
  expect(asked).toEqual([false, true])
  expect(group()?.getAttribute("data-coordinator-on")).toBe("true")
})

it("says a refused switch is not the person's to make", async () => {
  await switchShown(() => Promise.reject(new ChannelCreationError("forbidden")))
  await chooseCoordinator("Off")
  expect(host.querySelector("[role=alert]")?.textContent).toBe(
    "You can’t change this channel.",
  )
  expect(
    host.querySelector("[data-channel-coordinator]")?.getAttribute("data-coordinator-on"),
  ).toBe("true")
})
