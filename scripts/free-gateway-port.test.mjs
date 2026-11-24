import assert from "node:assert/strict"
import test from "node:test"

import { MOVE_RANGE, choosePort } from "./free-gateway-port.mjs"

/** A port table where only the listed ports are held, by something not ours. */
function scenario({
  held = [],
  gateways = [],
  explicit = false,
  move = true,
  reserved = [7420],
} = {}) {
  const said = []
  const asked = []
  return {
    said,
    asked,
    input: {
      wanted: 7421,
      explicit,
      move,
      reserved: new Set(reserved),
      stage: "dev",
      say: (line) => said.push(line),
      clear: async (port) => {
        asked.push(port)
        if (gateways.includes(port))
          return [{ text: `→ :${port} is a gateway of this namespace`, movable: false }]
        return held.includes(port) ? [{ text: `→ :${port} is held`, movable: true }] : []
      },
    },
  }
}

test("a free stage port is used as it is", async () => {
  const { input, said } = scenario()
  assert.equal(await choosePort(input), 7421)
  assert.deepEqual(said, [])
})

test("a stage port held by something not ours moves to the next free one and says so", async () => {
  const { input, said } = scenario({ held: [7421, 7422] })
  assert.equal(await choosePort(input), 7423)
  assert.match(said.join("\n"), /:7421 is held/)
  assert.match(said.join("\n"), /uses :7423 instead/)
  // The spare it skipped is not explained at length; only the wanted one is.
  assert.doesNotMatch(said.join("\n"), /:7422 is held/)
})

test("a moved port never lands on a port another stage is named for", async () => {
  const { input, asked } = scenario({ held: [7421], reserved: [7420, 7422] })
  assert.equal(await choosePort(input), 7423)
  assert.ok(!asked.includes(7422))
})

test("NESSA_PORT set by hand is refused, never moved", async () => {
  const { input, said } = scenario({ held: [7421], explicit: true })
  assert.equal(await choosePort(input), null)
  assert.match(said.join("\n"), /on purpose/)
})

test("without --move a held port is refused as before", async () => {
  const { input } = scenario({ held: [7421], move: false })
  assert.equal(await choosePort(input), null)
})

test("when every spare is held the run is refused with the range it tried", async () => {
  const held = Array.from({ length: MOVE_RANGE + 1 }, (_, index) => 7421 + index)
  const { input, said } = scenario({ held })
  assert.equal(await choosePort(input), null)
  assert.match(said.at(-1), new RegExp(`no free port in :7422–${7421 + MOVE_RANGE}`))
})

test("a gateway sharing this namespace is refused, not stepped around", async () => {
  const { input, said, asked } = scenario({ gateways: [7421] })
  assert.equal(await choosePort(input), null)
  assert.deepEqual(asked, [7421])
  assert.match(said.at(-1), /does not move/)
})

test("back on a free stage port, the spares are cleared of leftovers of ours", async () => {
  const { input, asked } = scenario()
  assert.equal(await choosePort(input), 7421)
  assert.deepEqual(asked.slice(0, 3), [7421, 7422, 7423])
  assert.equal(asked.length, 1 + MOVE_RANGE)
})

test("NESSA_PORT set by hand clears nothing but its own port", async () => {
  const { input, asked } = scenario({ explicit: true })
  assert.equal(await choosePort(input), 7421)
  assert.deepEqual(asked, [7421])
})
