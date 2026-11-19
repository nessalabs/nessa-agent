import { expect, it } from "vitest"
import { channelPlaceKind } from "./channel-place"

it("names each kind of place, and any other name as one computer", () => {
  expect(channelPlaceKind("local")).toBe("local")
  // Each cloud in the registry is a cloud place, by its own name.
  expect(channelPlaceKind("boat")).toBe("cloud")
  expect(channelPlaceKind("agent37")).toBe("cloud")
  expect(channelPlaceKind("pool")).toBe("pool")
  expect(channelPlaceKind("devbox")).toBe("computer")
  expect(channelPlaceKind(`peer:${"ab".repeat(32)}`)).toBe("computer")
})
