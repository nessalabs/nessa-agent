import { expect, it } from "vitest"
import { channelPlaceWords } from "./channel-place-words"

it("reads each place as a person would, and a computer by its best name", () => {
  expect(channelPlaceWords("local")).toBe("Local")
  expect(channelPlaceWords("boat")).toBe("Boat")
  expect(channelPlaceWords("pool")).toBe("Any computer")
  expect(channelPlaceWords("devbox")).toBe("devbox")
  // A paired computer by the start of its key, or by the name the caller has.
  expect(channelPlaceWords(`peer:${"ab".repeat(32)}`)).toBe("Paired machine abababab")
  expect(channelPlaceWords(`peer:${"ab".repeat(32)}`, "studio")).toBe("studio")
})
