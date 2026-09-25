import { expect, it } from "vitest"

import {
  composerHidden,
  dropShowsDraft,
  MESSAGES_TAB_ID,
  messagesAfter,
  stripOrder,
  tabClosed,
} from "./messages-tab"

it("puts an open Messages tab first and the update last", () => {
  expect(stripOrder("open", ["a", "b"], "nessa:update")).toEqual([
    MESSAGES_TAB_ID,
    "a",
    "b",
    "nessa:update",
  ])
  expect(stripOrder("viewing", ["a"], null)).toEqual([MESSAGES_TAB_ID, "a"])
})

it("leaves a closed Messages tab out of the strip", () => {
  expect(stripOrder("closed", ["a", "b"], null)).toEqual(["a", "b"])
})

it("keeps the Messages tab in the strip when another tab is chosen", () => {
  expect(messagesAfter("viewing", "leave")).toBe("open")
  expect(messagesAfter("open", "leave")).toBe("open")
  expect(messagesAfter("closed", "leave")).toBe("closed")
})

it("shows the Messages tab from any state, and closes it from any state", () => {
  for (const state of ["closed", "open", "viewing"] as const) {
    expect(messagesAfter(state, "show")).toBe("viewing")
    expect(messagesAfter(state, "close")).toBe("closed")
  }
})

it("closes whichever tab is on screen: the update, the Messages list, or the conversation", () => {
  expect(tabClosed(true, "viewing")).toBe("update")
  expect(tabClosed(false, "viewing")).toBe("messages")
  expect(tabClosed(false, "open")).toBe("conversation")
  expect(tabClosed(false, "closed")).toBe("conversation")
})

it("hides the composer while the update or the Messages list is on screen", () => {
  expect(composerHidden(false, "viewing")).toBe(true)
  expect(composerHidden(true, "closed")).toBe(true)
  expect(composerHidden(false, "open")).toBe(false)
})

it("shows a dropped-into draft only when it is the active one under the Messages list", () => {
  expect(dropShowsDraft("viewing", "a", "a")).toBe(true)
  // Answered late, into a draft that is no longer the active one.
  expect(dropShowsDraft("viewing", "b", "a")).toBe(false)
  // Already on screen: nothing to show.
  expect(dropShowsDraft("open", "a", "a")).toBe(false)
})
