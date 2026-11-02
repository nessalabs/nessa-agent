import assert from "node:assert/strict"
import { test } from "node:test"

import {
  agentsStudioMissingFailure,
  devicesCornerFailures,
  railViewCoverFailures,
  studioUnderToggleFailure,
} from "./rail-view-contract.mjs"

/** Toggle and words measured on main at 1440×900, columns layout. */
const toggle = [18, 851, 28, 28]
const studioOpen = [58, 851, 87, 28]
const studioClosed = [46, 851, 87, 28]

/**
 * Devices open, rail open, 1440×900. The titlebar's free room is the drag
 * region (`titlebarHit.inView` false). The old check treated that hit as the
 * workspace showing through.
 */
const covered = {
  view: true,
  top: 0,
  left: 64,
  right: 1440,
  bottom: 900,
  windowWidth: 1440,
  windowHeight: 900,
  railRight: 64,
  coversContent: true,
  hitWorkspace: false,
  titlebarHit: { inView: false, drag: true },
  navigation: ["side-rail"],
}

test("Agents keeps nessa Studio clear of the toggle, open rail and closed", () => {
  assert.deepEqual(studioUnderToggleFailure("open", studioOpen, toggle), [])
  assert.deepEqual(studioUnderToggleFailure("closed", studioClosed, toggle), [])
  assert.deepEqual(agentsStudioMissingFailure("shown", studioOpen), [])
  assert.deepEqual(agentsStudioMissingFailure("shown", studioClosed), [])
})

test("a missing Agents word, or words under the toggle, fails", () => {
  assert.deepEqual(agentsStudioMissingFailure("shown", null), [
    'shown: "nessa Studio" is missing in Agents',
  ])
  assert.deepEqual(studioUnderToggleFailure("open", [40, 851, 87, 28], toggle), [
    'open: "nessa Studio" at x 40 is under the toggle',
  ])
})

test("Devices leaves the corner clear and the toggle unmoved", () => {
  assert.deepEqual(
    devicesCornerFailures("rail open, sidebar shown", null, toggle, toggle),
    [],
  )
  assert.deepEqual(
    devicesCornerFailures("rail open, sidebar shown", studioOpen, toggle, toggle),
    [
      'rail open, sidebar shown: Devices draws "nessa Studio" at [58,851,87,28]; that corner stays clear',
    ],
  )
  assert.deepEqual(
    devicesCornerFailures("rail open, sidebar shown", null, [20, 851, 28, 28], toggle),
    [
      "rail open, sidebar shown, Devices: the toggle is at [20,851,28,28], not [18,851,28,28]",
    ],
  )
})

test("a covered Devices view is not exposure when the titlebar is the drag region", () => {
  assert.deepEqual(railViewCoverFailures(covered), [])
})

test("a gap, a workspace hit, or a workspace control in the titlebar is exposure", () => {
  const message = "plugin view exposes the workspace cards or controls above it"
  assert.deepEqual(railViewCoverFailures({ ...covered, top: 48 }), [message])
  assert.deepEqual(railViewCoverFailures({ ...covered, left: 0 }), [message])
  assert.deepEqual(
    railViewCoverFailures({ ...covered, coversContent: false, hitWorkspace: true }),
    [message],
  )
  assert.deepEqual(
    railViewCoverFailures({ ...covered, navigation: ["workspace-sidebar"] }),
    [message],
  )
  assert.deepEqual(
    railViewCoverFailures({ view: false, titlebarHit: covered.titlebarHit }),
    [message],
  )
})
