import { describe, expect, it, test } from "vitest"

import { browser } from "./browser"
import { linux } from "./linux"
import { macos } from "./macos"
import { other } from "./other"
import { resolveHost } from "./resolve"
import {
  WEST_HANDLE_NARROW,
  WEST_HANDLE_WIDE,
  type CompositorKind,
  type FrostKind,
  type HostFeatures,
  type HostKind,
} from "./features"

function expectFeatures(
  host: HostFeatures,
  expected: {
    kind: HostKind
    frost: FrostKind
    compositor: CompositorKind
    westHandleClass: string
    capturePointerOnWestHandle: boolean
    animateMount: boolean
    streamText: boolean
    emptyState: boolean
    flushOnTurn: boolean
  },
) {
  expect(host).toEqual(expected)
}

describe("host features", () => {
  test("linux is a layout compositor: no mount springs, no streamed tokens", () => {
    expectFeatures(linux, {
      kind: "linux",
      frost: "css",
      compositor: "layout",
      westHandleClass: WEST_HANDLE_WIDE,
      capturePointerOnWestHandle: false,
      animateMount: false,
      streamText: false,
      emptyState: true,
      flushOnTurn: true,
    })
  })

  test("macos is a layer compositor: mount springs and streamed tokens", () => {
    expectFeatures(macos, {
      kind: "macos",
      frost: "native",
      compositor: "layer",
      westHandleClass: WEST_HANDLE_NARROW,
      capturePointerOnWestHandle: true,
      animateMount: true,
      streamText: true,
      emptyState: true,
      flushOnTurn: false,
    })
  })

  test("browser matches the layer compositor policy", () => {
    expectFeatures(browser, {
      kind: "browser",
      frost: "css",
      compositor: "layer",
      westHandleClass: WEST_HANDLE_NARROW,
      capturePointerOnWestHandle: true,
      animateMount: true,
      streamText: true,
      emptyState: true,
      flushOnTurn: false,
    })
  })

  test("other matches the layer compositor policy", () => {
    expectFeatures(other, {
      kind: "other",
      frost: "css",
      compositor: "layer",
      westHandleClass: WEST_HANDLE_NARROW,
      capturePointerOnWestHandle: true,
      animateMount: true,
      streamText: true,
      emptyState: true,
      flushOnTurn: false,
    })
  })
})

describe("resolveHost", () => {
  it("injects the browser host when there is no Tauri runtime", () => {
    expect(resolveHost("Mozilla/5.0 (Macintosh; Intel Mac OS X)", false)).toBe(browser)
  })

  it("injects macOS features for a Mac Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (Macintosh; Intel Mac OS X)", true)).toBe(macos)
  })

  it("injects Linux features for a Linux Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (X11; Linux x86_64)", true)).toBe(linux)
  })

  it("injects the other host for an unmatched Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", true)).toBe(other)
  })
})
