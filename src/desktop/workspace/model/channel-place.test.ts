import { expect, it } from "vitest"
import { channelPlaceKind, channelPlaceWords } from "./channel-place"

it("names each kind of place, and any other name as one computer", () => {
  expect(channelPlaceKind("local")).toBe("local")
  expect(channelPlaceKind("boat")).toBe("boat")
  expect(channelPlaceKind("pool")).toBe("pool")
  expect(channelPlaceKind("devbox")).toBe("computer")
  expect(channelPlaceKind(`peer:${"ab".repeat(32)}`)).toBe("computer")
  expect(channelPlaceWords("local")).toBe("Local")
  expect(channelPlaceWords("pool")).toBe("Any computer")
  expect(channelPlaceWords("devbox")).toBe("devbox")
  // A paired computer by the start of its key, or by the name the caller has.
  expect(channelPlaceWords(`peer:${"ab".repeat(32)}`)).toBe("Paired machine abababab")
  expect(channelPlaceWords(`peer:${"ab".repeat(32)}`, "studio")).toBe("studio")
})
