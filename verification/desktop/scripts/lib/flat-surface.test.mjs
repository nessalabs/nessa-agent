import assert from "node:assert/strict"
import { test } from "node:test"
import { runInNewContext } from "node:vm"
import { flatSurfaceFailures } from "./workspace.mjs"

const clear = "rgba(0, 0, 0, 0)"
const rect = (left, top, right, bottom) => ({
  left,
  top,
  right,
  bottom,
  toJSON: () => ({ left, top, right, bottom }),
})
function check(damage = {}) {
  const bounds = rect(0, 0, 1000, 700)
  let boxes = damage.threeRows
    ? [rect(8, 8, 992, 228), rect(8, 236, 992, 456), rect(8, 464, 992, 692)]
    : [rect(8, 8, 496, 692), rect(504, 8, 992, 346), rect(504, 354, 992, 692)]
  if (damage.firstColumn)
    boxes = [rect(8, 8, 496, 346), rect(8, 354, 496, 692), rect(504, 8, 992, 692)]
  if (damage.staggered)
    boxes = [
      rect(8, 8, 496, 340),
      rect(8, 348, 496, 692),
      rect(504, 8, 992, 346),
      rect(504, 354, 992, 692),
    ]
  const panes = boxes.map((box) => ({ getBoundingClientRect: () => box }))
  const column = {
    dataset: { axis: "x" },
    getBoundingClientRect: () => rect(496, 8, 504, 692),
    after: {
      content: '""',
      backgroundColor: "rgb(30, 30, 30)",
      width: "1px",
      left: "3.5px",
      top: "-8px",
      bottom: "-8px",
    },
  }
  const row = {
    dataset: { axis: "y" },
    getBoundingClientRect: () =>
      damage.threeRows
        ? rect(8, 228, 992, 236)
        : damage.firstColumn
          ? rect(8, 346, 496, 354)
          : rect(504, 346, 992, 354),
    after: {
      content: '""',
      backgroundColor: "rgb(30, 30, 30)",
      height: "1px",
      left: damage.threeRows || damage.firstColumn ? "-8px" : "-4px",
      right: damage.firstColumn ? "-4px" : "-8px",
      top: "3.5px",
      ...damage.row,
    },
  }
  const edges = damage.threeRows ? [row] : [column, row]
  const surface = { getBoundingClientRect: () => bounds, querySelectorAll: () => edges }
  const document = { querySelector: () => surface, querySelectorAll: () => panes }
  const getComputedStyle = (element, pseudo) =>
    pseudo
      ? element.after
      : element === surface
        ? { backgroundColor: damage.noFill ? clear : "rgb(200, 200, 200)" }
        : {
            backgroundColor: clear,
            boxShadow: damage.card ? "inset 0 0 0 1px black" : "none",
          }
  return flatSurfaceFailures({
    evaluate: (fn, argument) =>
      runInNewContext(`(${fn})(argument)`, {
        argument,
        document,
        getComputedStyle,
        innerWidth: 1000,
        innerHeight: 700,
      }),
  })
}
for (const [name, damage] of [
  ["T-shaped", {}],
  ["first-column T-shaped", { firstColumn: true }],
  ["staggered adjacent columns", { staggered: true }],
  ["first adjoining row", { threeRows: true }],
  ["later adjoining row", { threeRows: true, row: { top: "231.5px" } }],
])
  test(`flat surface accepts ${name} gutter`, () => assert.equal(check(damage).length, 0))
for (const [name, damage] of [
  ["shifted row", { row: { top: "-96.5px" } }],
  ["gap from another column", { staggered: true, row: { top: "0.5px" } }],
  ["row over a nonadjacent gap", { threeRows: true, row: { top: "117.5px" } }],
  ["row across another column", { row: { left: "-504px" } }],
  [
    "first-column row across another column",
    { firstColumn: true, row: { right: "-504px" } },
  ],
  ["row stopping at the wrong side", { row: { right: "0px" } }],
  ["thick row", { row: { height: "2px" } }],
  ["card shadow", { card: true }],
  ["missing fill", { noFill: true }],
])
  test(`flat surface rejects ${name}`, () => assert.ok(check(damage).length > 0))
