import { describe, expect, it } from "vitest"
import {
  columnToToggle,
  GESTURE_GAP,
  idleSwipe,
  swipeStep,
  SWIPE_DISTANCE,
  swipeColumnsOf,
  swipeReads,
  type SwipeColumns,
  type SwipeDirection,
  type SwipeSample,
  type SwipeState,
} from "./side-swipe"

const all: SwipeColumns = { rail: "open", sidebar: "open", list: "open" }

describe("columnToToggle", () => {
  it("folds the open column nearest the chat on a swipe left, one at a time", () => {
    expect(columnToToggle(all, "left")).toBe("list")
    expect(columnToToggle({ ...all, list: "closed" }, "left")).toBe("sidebar")
    expect(
      columnToToggle({ rail: "open", sidebar: "closed", list: "closed" }, "left"),
    ).toBe("rail")
    expect(
      columnToToggle({ rail: "closed", sidebar: "closed", list: "closed" }, "left"),
    ).toBeNull()
  })

  it("opens the closed column nearest the window's edge on a swipe right", () => {
    const none: SwipeColumns = { rail: "closed", sidebar: "closed", list: "closed" }
    expect(columnToToggle(none, "right")).toBe("rail")
    expect(columnToToggle({ ...none, rail: "open" }, "right")).toBe("sidebar")
    expect(columnToToggle({ ...all, list: "closed" }, "right")).toBe("list")
    // Everything open but the rail: the rail is what a swipe right brings.
    expect(columnToToggle({ ...all, rail: "closed" }, "right")).toBe("rail")
    expect(columnToToggle(all, "right")).toBeNull()
  })

  it("passes over a column the window cannot draw", () => {
    const noRail: SwipeColumns = { rail: "absent", sidebar: "closed", list: "absent" }
    expect(columnToToggle(noRail, "right")).toBe("sidebar")
    expect(columnToToggle({ ...noRail, sidebar: "open" }, "left")).toBe("sidebar")
    expect(columnToToggle({ ...noRail, sidebar: "open" }, "right")).toBeNull()
  })
})

describe("swipeColumnsOf", () => {
  const drawn = {
    railOffered: true,
    railDrawn: true,
    railRoom: true,
    sidebarOpen: true,
    listOffered: true,
    listOpen: true,
  }

  it("reads what the window draws", () => {
    expect(swipeColumnsOf(drawn)).toEqual(all)
    expect(
      swipeColumnsOf({ ...drawn, railDrawn: false, sidebarOpen: false, listOpen: false }),
    ).toEqual({ rail: "closed", sidebar: "closed", list: "closed" })
  })

  it("holds the rail absent while its preview is off or it has no room", () => {
    expect(swipeColumnsOf({ ...drawn, railOffered: false }).rail).toBe("absent")
    expect(swipeColumnsOf({ ...drawn, railDrawn: false, railRoom: false }).rail).toBe(
      "absent",
    )
  })

  it("holds the list absent in a layout without one", () => {
    expect(swipeColumnsOf({ ...drawn, listOffered: false, listOpen: false }).list).toBe(
      "absent",
    )
  })
})

describe("swipeReads", () => {
  const free = { browserSurface: false, fullView: false, panelMaximized: false }

  it("reads in the app's window with the panes in sight", () => {
    expect(swipeReads(free)).toBe(true)
  })

  it("reads nothing in a browser tab or with the panes covered", () => {
    expect(swipeReads({ ...free, browserSurface: true })).toBe(false)
    expect(swipeReads({ ...free, fullView: true })).toBe(false)
    expect(swipeReads({ ...free, panelMaximized: true })).toBe(false)
  })
})

/** Plays samples through the recognizer; returns each swipe it reported. */
function play(samples: readonly SwipeSample[], skip = false) {
  let state: SwipeState = idleSwipe
  const swiped: SwipeDirection[] = []
  for (const sample of samples) {
    const next = swipeStep(state, sample, skip)
    state = next.state
    if (next.swiped) swiped.push(next.swiped)
  }
  return { state, swiped }
}

/** A gesture of `count` events 16 ms apart from `start`, each moving (x, y). */
const gesture = (start: number, count: number, x: number, y = 0): SwipeSample[] =>
  Array.from({ length: count }, (_, i) => ({ at: start + i * 16, x, y }))

describe("swipeStep", () => {
  it("reports one swipe for a fling, momentum included", () => {
    // 30 events of 10 px: 300 px in all, far past the distance.
    expect(play(gesture(0, 30, 10)).swiped).toEqual(["left"])
    expect(play(gesture(0, 30, -10)).swiped).toEqual(["right"])
  })

  it("waits for the distance", () => {
    const short = Math.floor((SWIPE_DISTANCE - 1) / 4)
    expect(play(gesture(0, short, 4)).swiped).toEqual([])
  })

  it("reports a second swipe after a pause", () => {
    const first = gesture(0, 10, 10)
    const later = (first.at(-1)?.at ?? 0) + GESTURE_GAP + 1
    expect(play([...first, ...gesture(later, 10, -10)]).swiped).toEqual(["left", "right"])
  })

  it("reads the next swipe once the last one's momentum has settled, with no pause", () => {
    // A fling whose momentum dies down to nothing, then fresh fingers at once.
    const fling = gesture(0, 6, 15)
    const tail = [8, 4, 2, 1, 0.5].map((x, i) => ({ at: 96 + i * 16, x, y: 0 }))
    const again = gesture(96 + 5 * 16, 6, 15)
    expect(play([...fling, ...tail, ...again]).swiped).toEqual(["left", "left"])
  })

  it("keeps momentum that has not settled as the same swipe", () => {
    const fling = gesture(0, 6, 15)
    const tail = [12, 10, 9, 7, 6, 5, 4, 3, 2].map((x, i) => ({
      at: 96 + i * 16,
      x,
      y: 0,
    }))
    expect(play([...fling, ...tail]).swiped).toEqual(["left"])
  })

  it("reads a swipe the other way at once, momentum or not", () => {
    const fling = gesture(0, 6, 15)
    const back = gesture(96, 6, -15)
    expect(play([...fling, ...back]).swiped).toEqual(["left", "right"])
  })

  it("reads a swipe right after a vertical scroll's momentum settles", () => {
    const scroll = [...gesture(0, 6, 0, 20), { at: 96, x: 0, y: 1 }]
    expect(play([...scroll, ...gesture(112, 6, -15)]).swiped).toEqual(["right"])
  })

  it("leaves a vertical scroll alone, even when it drifts sideways later", () => {
    const scroll = [...gesture(0, 4, 0, 6), ...gesture(64, 20, 12, 0)]
    expect(play(scroll).swiped).toEqual([])
    expect(play(scroll).state.phase).toBe("scroll")
  })

  it("does not count a diagonal", () => {
    expect(play(gesture(0, 20, 6, 4)).swiped).toEqual([])
  })

  it("leaves a gesture alone that began over something scrolling sideways", () => {
    const result = play(gesture(0, 30, 10), true)
    expect(result.swiped).toEqual([])
    expect(result.state.phase).toBe("skipped")
  })

  it("decides skipping where the gesture began, not on each event", () => {
    let state: SwipeState = idleSwipe
    const swiped: (SwipeDirection | null)[] = []
    gesture(0, 10, 10).forEach((sample, i) => {
      // Only the first event's answer counts: the pointer passing over a
      // table mid-swipe does not stop it.
      const next = swipeStep(state, sample, i > 0)
      state = next.state
      swiped.push(next.swiped)
    })
    expect(swiped.filter(Boolean)).toEqual(["left"])
  })
})
