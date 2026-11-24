// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { Agent37Status, BoatStatus, Cloud } from "@nessa/client"
import type { Agent37Gateway } from "../../settings/adapters/agent37-gateway"
import type { BoatGateway } from "../../settings/adapters/boat-gateway"
import { Agent37Provider } from "../../settings/ui/agent37-row"
import { BoatProvider } from "../../settings/ui/boat-row"
import { DefaultCloudRow } from "./default-cloud-row"

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
const choice = () => host.querySelector<HTMLElement>("[data-cloud-choice]")

function gateways(boat: BoatStatus["state"], agent37: Agent37Status["state"]) {
  let cloud: Cloud = "boat"
  const boatGateway = { read: async () => ({ state: boat }) } as unknown as BoatGateway
  const agent37Gateway: Agent37Gateway = {
    read: async () => ({ state: agent37 }),
    save: async () => ({ state: agent37 }),
    disconnect: async () => ({ state: "disconnected" }),
    cloud: async () => ({ cloud }),
    chooseCloud: async (next) => {
      cloud = next
      return { cloud }
    },
  }
  return { boatGateway, agent37Gateway }
}

async function render(boat: BoatStatus["state"], agent37: Agent37Status["state"]) {
  const { boatGateway, agent37Gateway } = gateways(boat, agent37)
  await act(async () =>
    root.render(
      <BoatProvider gateway={boatGateway}>
        <Agent37Provider gateway={agent37Gateway}>
          <DefaultCloudRow />
        </Agent37Provider>
      </BoatProvider>,
    ),
  )
  await act(async () => {})
}

it("is not offered while only one cloud is connected", async () => {
  await render("disconnected", "connected")
  expect(choice()).toBeNull()
  await render("connected", "disconnected")
  expect(choice()).toBeNull()
})

it("offers the default cloud once two are connected", async () => {
  await render("connected", "connected")
  expect(choice()?.dataset.cloudChoice).toBe("boat")
})
