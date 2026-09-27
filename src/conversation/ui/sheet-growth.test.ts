import { expect, it } from "vitest"
import { durationMilliseconds, sheetGrowth } from "./sheet-growth"

/**
 * A panel whose state the test moves by hand. `height` is where the layout
 * puts it; while a growth is live the panel is on screen at `onScreen`
 * instead, and cancelling the growth lets the layout show through — as a real
 * animation overriding `height` does.
 */
function fakePanel() {
  const state = {
    height: 288,
    onScreen: 288,
    width: 420,
    expanded: false,
    held: false,
    motion: true,
    animations: [] as { from: number; to: number; cancelled: boolean }[],
  }
  const live = () => state.animations.some((animation) => !animation.cancelled)
  const panel = {
    height: () => (live() ? state.onScreen : state.height),
    width: () => state.width,
    expanded: () => state.expanded,
    heldBySheet: () => state.held,
    animate(from: number, to: number) {
      if (!state.motion) return null
      const animation = { from, to, cancelled: false }
      state.animations.push(animation)
      return {
        cancel() {
          animation.cancelled = true
        },
      }
    },
  }
  return { state, growth: sheetGrowth(panel) }
}

it("grows from where the panel was to where the layout now is", () => {
  const { state, growth } = fakePanel()
  state.height = 330
  growth.contentChanged()
  expect(state.animations).toEqual([{ from: 288, to: 330, cancelled: false }])
})

it("continues from the panel's height on screen when a tool arrives mid-growth", () => {
  const { state, growth } = fakePanel()
  state.height = 330
  growth.contentChanged()
  // Partway through, the panel is on screen at 300, and a second tool lands.
  state.onScreen = 300
  state.height = 370
  growth.contentChanged()
  expect(state.animations).toEqual([
    { from: 288, to: 330, cancelled: true },
    // Not from 330, where the first growth was headed: that would jump.
    { from: 300, to: 370, cancelled: false },
  ])
})

it("leaves a panel the Sheet is moving to the Sheet", () => {
  const { state, growth } = fakePanel()
  state.held = true
  state.height = 400
  growth.contentChanged()
  expect(state.animations).toEqual([])
})

it("does not grow an expanded panel, whose content scrolls instead", () => {
  const { state, growth } = fakePanel()
  state.expanded = true
  state.height = 700
  growth.contentChanged()
  expect(state.animations).toEqual([])
})

it("treats a reflow from a width change as the window moving, not the turn growing", () => {
  const { state, growth } = fakePanel()
  state.width = 360
  state.height = 340
  growth.contentChanged()
  expect(state.animations).toEqual([])
})

it("keeps up with the panel moving on its own, so the next growth starts from the truth", () => {
  const { state, growth } = fakePanel()
  // A drag or toggle left the panel at 500, and the Sheet has let go of it.
  state.height = 500
  growth.panelChanged()
  state.height = 540
  growth.contentChanged()
  expect(state.animations).toEqual([{ from: 500, to: 540, cancelled: false }])
})

it("snaps when motion is off, and still tracks where it landed", () => {
  const { state, growth } = fakePanel()
  state.motion = false
  state.height = 330
  growth.contentChanged()
  state.motion = true
  state.height = 360
  growth.contentChanged()
  expect(state.animations).toEqual([{ from: 330, to: 360, cancelled: false }])
})

it("reads the design system's duration tokens, and reads anything else as stillness", () => {
  expect(durationMilliseconds("200ms")).toBe(200)
  expect(durationMilliseconds(" 0.3s ")).toBe(300)
  // Reduced motion collapses the token to zero.
  expect(durationMilliseconds("0s")).toBe(0)
  expect(durationMilliseconds("")).toBe(0)
  expect(durationMilliseconds("fast")).toBe(0)
  expect(durationMilliseconds("-1s")).toBe(0)
})
