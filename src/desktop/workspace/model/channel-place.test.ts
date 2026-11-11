import { expect, it } from "vitest"
import { channelPlaceKind } from "./channel-place"

it("names each kind of place, and any other name as one computer", () => {
  expect(channelPlaceKind("local")).toBe("local")
  expect(channelPlaceKind("boat")).toBe("boat")
  expect(channelPlaceKind("pool")).toBe("pool")
  expect(channelPlaceKind("devbox")).toBe("computer")
  expect(channelPlaceKind(`peer:${"ab".repeat(32)}`)).toBe("computer")
})
