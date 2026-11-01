import assert from "node:assert/strict"
import { test } from "node:test"
import { hoverFailures, sampleHover } from "./hover-state.mjs"

const hover = "oklab(0.985 0 0 / 0.05)"
const held = {
  fill: hover,
  connected: true,
  inputWithinRow: true,
  controlWithinRow: true,
  pointerWithinInput: true,
  hovered: true,
  pointerWithinRow: true,
  finiteActive: 0,
}
test("only the exact settled ink under the same pointer can pass", () => {
  assert.deepEqual(hoverFailures(held, [hover]), [])
  for (const change of [
    { fill: "oklab(0.985 0 0 / 0.0499794)" },
    { fill: "oklab(0.985 0 0 / 0.03)" },
    { connected: false },
    { inputWithinRow: false },
    { controlWithinRow: false },
    { pointerWithinInput: false },
    { hovered: false },
    { pointerWithinRow: false },
    { finiteActive: 1 },
  ])
    assert.notEqual(hoverFailures({ ...held, ...change }, [hover]).length, 0)
})

test("contradictory pointer and action owners are rejected before browser effects", async () => {
  for (const options of [{ pointerOwner: "other" }, { pointerOwner: "frame" }])
    await assert.rejects(sampleHover(null, null, options), /hover pointer owner/)
  await assert.rejects(
    sampleHover(null, null, { actions: ".actions" }),
    /action hits require/,
  )
})

test(
  "sibling action hits and hover proof stay within their pinned row",
  { skip: !process.env.NESSA_HOVER_BROWSER },
  async () => {
    const { chromium } = await import("playwright")
    const browser = await chromium.launch({
      executablePath:
        process.env.NESSA_HOVER_BROWSER === "bundled"
          ? undefined
          : process.env.NESSA_HOVER_BROWSER,
      headless: true,
      args: ["--no-sandbox"],
    })
    try {
      const page = await browser.newPage({ viewport: { width: 320, height: 160 } })
      const markup = `<!doctype html><style>
        body { margin: 0 }
        #frame { position: absolute; left: 30px; top: 30px; width: 200px; height: 40px }
        #control { position: absolute; inset: 0; border: 0; background: rgb(30,30,30) }
        #frame:hover #control { background: rgb(60,60,60) }
        #mark, #actions { position: absolute; right: 0; top: 0; width: 40px; height: 40px }
        #mark { pointer-events: none }
        #actions { pointer-events: none; opacity: 0 }
        #frame:hover #actions { pointer-events: auto; opacity: 1 }
      </style><div id="frame" data-row=""><button id="control">Row</button>
        <span id="mark">3</span><button id="actions">Actions</button></div>`
      const options = { frame: "[data-row]", row: "#control", force: true }
      const frameOptions = { ...options, pointerOwner: "frame", actions: "#actions" }
      const sample = () => sampleHover(page, page.locator("#mark"), frameOptions)
      const failures = (value) => hoverFailures(value.settled, ["rgb(60, 60, 60)"])
      await page.setContent(markup)
      const control = await sampleHover(page, page.locator("#mark"), options)
      assert.notDeepEqual(failures(control), [])
      const frame = await sample()
      assert.deepEqual(failures(frame), [], JSON.stringify(frame))
      assert.equal(frame.settled.hit, "actions")
      assert.equal(frame.away.hit, "elsewhere")
      assert.equal(frame.away.hovered, false)
      assert.equal(frame.away.pointerWithinRow, false)
      assert.equal(frame.away.pointerWithinInput, false)

      await page.setContent(markup)
      await page.locator("#actions").evaluate((element) => element.remove())
      const plain = await sample()
      assert.deepEqual(failures(plain), [])
      assert.equal(plain.settled.hit, "row")
      const ordinary = await sampleHover(page, page.locator("#control"))
      assert.deepEqual(failures(ordinary), [])

      // A nested row's actions do not become this row's actions.
      await page.setContent(markup)
      await page.locator("#actions").evaluate((element) => element.remove())
      await page.evaluate(() => {
        const frame = document.getElementById("frame")
        const nested = frame.appendChild(document.createElement("div"))
        nested.setAttribute("data-row", "")
        Object.assign(nested.style, {
          position: "absolute",
          left: "0",
          top: "0",
          width: "40px",
          height: "20px",
        })
        const action = nested.appendChild(document.createElement("button"))
        action.id = "actions"
      })
      const nestedAction = await sample()
      assert.deepEqual(failures(nestedAction), [])
      assert.equal(nestedAction.settled.hit, "row")

      // Keep every other fact valid while the original control leaves the frame.
      await page.setContent(markup)
      await page.evaluate(() => {
        const outside = document.body.appendChild(document.createElement("div"))
        const control = document.getElementById("control")
        control.style.background = "rgb(60,60,60)"
        control.style.position = "static"
        control.style.pointerEvents = "none"
        document
          .getElementById("frame")
          .addEventListener("pointerenter", () => outside.append(control), { once: true })
      })
      const movedControl = await sample()
      assert.equal(movedControl.settled.connected, true)
      assert.deepEqual(failures(movedControl), [
        "the sampled control left its pinned row",
      ])

      // The input can keep its screen position but belong to a different row.
      await page.setContent(markup)
      await page.evaluate(() => {
        const other = document.body.appendChild(document.createElement("div"))
        Object.assign(other.style, {
          position: "absolute",
          left: "30px",
          top: "30px",
          width: "200px",
          height: "40px",
          pointerEvents: "none",
        })
        const mark = document.getElementById("mark")
        document
          .getElementById("frame")
          .addEventListener("pointerenter", () => other.append(mark), { once: true })
      })
      const movedInput = await sample()
      assert.equal(movedInput.settled.hit, "actions")
      assert.equal(movedInput.settled.pointerWithinInput, true)
      assert.deepEqual(failures(movedInput), ["the hover input left its pinned row"])

      // An ancestor is not the input's canonical row after a nested row takes it.
      await page.setContent(markup)
      await page.evaluate(() => {
        const frame = document.getElementById("frame")
        const nested = frame.appendChild(document.createElement("div"))
        nested.setAttribute("data-row", "")
        Object.assign(nested.style, {
          position: "absolute",
          inset: "0",
          pointerEvents: "none",
        })
        const mark = document.getElementById("mark")
        frame.addEventListener("pointerenter", () => nested.append(mark), { once: true })
      })
      const nestedInput = await sample()
      assert.equal(nestedInput.settled.pointerWithinInput, true)
      assert.equal(nestedInput.settled.hit, "actions")
      assert.deepEqual(failures(nestedInput), ["the hover input left its pinned row"])

      await page.setContent(markup)
      await page.evaluate(() => {
        const frame = document.getElementById("frame")
        const nested = frame.appendChild(document.createElement("div"))
        nested.setAttribute("data-row", "")
        nested.style.pointerEvents = "none"
        const control = document.getElementById("control")
        Object.assign(control.style, {
          position: "static",
          pointerEvents: "none",
          background: "rgb(60,60,60)",
        })
        frame.addEventListener("pointerenter", () => nested.append(control), {
          once: true,
        })
      })
      const nestedControl = await sample()
      assert.deepEqual(failures(nestedControl), [
        "the sampled control left its pinned row",
      ])

      // A nested row cannot drive its ancestor's proof even with input/control unmoved.
      await page.setContent(markup)
      await page.evaluate(() => {
        const frame = document.getElementById("frame")
        frame.addEventListener(
          "pointerenter",
          () => {
            const nested = frame.appendChild(document.createElement("div"))
            nested.setAttribute("data-row", "")
            Object.assign(nested.style, { position: "absolute", inset: "0" })
          },
          { once: true },
        )
      })
      const nestedPointer = await sample()
      assert.equal(nestedPointer.settled.inputWithinRow, true)
      assert.equal(nestedPointer.settled.controlWithinRow, true)
      assert.equal(nestedPointer.settled.pointerWithinInput, true)
      assert.equal(nestedPointer.settled.hit, "elsewhere")
      assert.deepEqual(failures(nestedPointer), ["the pointer left the sampled row"])

      // Remaining in the row does not prove the pointer still covers the input.
      await page.setContent(markup)
      await page.evaluate(() => {
        const mark = document.getElementById("mark")
        document.getElementById("frame").addEventListener(
          "pointerenter",
          () => {
            mark.style.left = "0"
            mark.style.right = "auto"
          },
          { once: true },
        )
      })
      const shifted = await sample()
      assert.equal(shifted.settled.inputWithinRow, true)
      assert.equal(shifted.settled.hit, "actions")
      assert.deepEqual(failures(shifted), ["the pointer left the pinned hover input"])
    } finally {
      await browser.close()
    }
  },
)
