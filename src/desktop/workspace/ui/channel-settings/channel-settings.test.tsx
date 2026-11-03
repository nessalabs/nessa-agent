// @vitest-environment jsdom
/**
 * Where a made channel runs, changed in its settings: Boat waits for a
 * connection, a change in flight holds the rows, and a refusal says why.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { BoatStatus } from "@nessa/client"
import { makeDesktopStore } from "../../../store"
import type { BoatGateway } from "../../../settings/adapters/boat-gateway"
import { BoatProvider } from "../../../settings/ui/boat-row"
import { createQuestionFlights } from "../../application/question-flights"
import {
  ChannelCreationError,
  type ChannelPlacement,
  type ChannelSource,
} from "../../application/ports"
import type { Channel } from "../../model/workspace-index"
import { loadWorkspace } from "../../adapters/store/commands"
import { fakeSource, keptFilter, roomyGrid, settle, testIndex } from "../../testing"
import { ChannelPlacementControl } from "./channel-settings"

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

// Each test starts from its own index: a move changes the one it is shown on.
let index = testIndex()
let channel = index.channels[0] as Channel

function boat(state: BoatStatus["state"]): BoatGateway {
  const status = { state, environment: "nessa", image: "fixture" } as BoatStatus
  return {
    read: async () => status,
    save: async () => status,
    disconnect: async () => status,
    runtimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
    setRuntimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
  }
}

async function shown(
  setPlacement: (id: string, placement: ChannelPlacement) => Promise<Channel>,
  boatState: BoatStatus["state"] = "connected",
) {
  index = testIndex()
  channel = index.channels.find((each) => each.id !== "default") as Channel
  const channels = {
    create: () => Promise.reject(new Error("unused")),
    setPlacement,
  } as ChannelSource
  let next = 0
  const store = makeDesktopStore({
    workspace: fakeSource(index),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    channels,
  })
  await store.dispatch(loadWorkspace())
  await settle()
  await act(async () => {
    root.render(
      <Provider store={store}>
        <BoatProvider gateway={boat(boatState)}>
          <ChannelPlacementControl channelId={channel.id} />
        </BoatProvider>
      </Provider>,
    )
  })
  await act(settle)
}

const option = (row: string, label: string) => {
  const found = [...host.querySelectorAll(`[data-placement-row="${row}"] button`)].find(
    (each) => each.textContent === label,
  )
  if (!(found instanceof HTMLButtonElement)) throw new Error(`no ${label} in ${row}`)
  return found
}

it("offers Boat only once it is connected", async () => {
  await shown(async () => channel, "disconnected" as BoatStatus["state"])
  expect(option("threads", "Boat (not connected)").disabled).toBe(true)
  expect(option("threads", "This Mac").disabled).toBe(false)
})

it("holds both rows while a change is on its way, then shows it", async () => {
  const asked: ChannelPlacement[] = []
  let land!: () => void
  await shown(
    (_id, placement) =>
      new Promise((resolve) => {
        asked.push(placement)
        land = () => {
          // The source lists it moved too, as the gateway's list does.
          const moved = { ...channel, ...placement }
          const listed = index.channels as Channel[]
          listed[listed.indexOf(channel)] = moved
          resolve(moved)
        }
      }),
  )
  await act(async () => option("threads", "Boat").click())
  expect(asked).toEqual([{ coordinatorPlacement: "local", threadPlacement: "boat" }])
  expect(option("coordinator", "Boat").disabled).toBe(true)
  await act(async () => option("coordinator", "Boat").click())
  expect(asked).toHaveLength(1)
  await act(async () => {
    land()
    await settle()
  })
  const group = host.querySelector("[data-channel-placement]")
  expect(group?.getAttribute("data-thread-placement")).toBe("boat")
  expect(option("coordinator", "Boat").disabled).toBe(false)
})

it("says a refusal is not the person's to make, and any other failure is passing", async () => {
  await shown(() => Promise.reject(new ChannelCreationError("forbidden")))
  await act(async () => option("threads", "Boat").click())
  await act(settle)
  expect(host.querySelector("[role=alert]")?.textContent).toBe(
    "You can’t change where this channel runs.",
  )
  await shown(() => Promise.reject(new Error("offline")))
  await act(async () => option("threads", "Boat").click())
  await act(settle)
  expect(host.querySelector("[role=alert]")?.textContent).toBe(
    "Couldn’t save that. Try again in a moment.",
  )
})
