import { expect, it } from "vitest"
import { cloudChoices, cloudProvider, clouds, isCloud } from "./clouds"

it("lists every cloud once, in the order the pickers offer them", () => {
  expect(clouds.map((cloud) => cloud.id)).toEqual(["boat", "agent37"])
  for (const cloud of clouds) expect(cloudProvider(cloud.id)).toBe(cloud)
})

it("knows a cloud's word and no other, not even an inherited key", () => {
  expect(isCloud("boat")).toBe(true)
  expect(isCloud("agent37")).toBe(true)
  expect(isCloud("pool")).toBe(false)
  expect(isCloud("devbox")).toBe(false)
  expect(isCloud("toString")).toBe(false)
})

it("offers each cloud, disabling one not connected and saying so when asked", () => {
  const connected = (id: string) => id === "agent37"
  expect(cloudChoices(connected)).toEqual([
    { id: "boat", label: "Boat", disabled: true },
    { id: "agent37", label: "Agent37", disabled: false },
  ])
  expect(cloudChoices(connected, "with-state")).toEqual([
    { id: "boat", label: "Boat (not connected)", disabled: true },
    { id: "agent37", label: "Agent37", disabled: false },
  ])
})
