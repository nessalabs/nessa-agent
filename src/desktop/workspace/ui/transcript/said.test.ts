/**
 * How a command and a name are shown (#553): bidi controls are visible, and
 * a JSON argument still parses to the argument that runs.
 */
import { describe, expect, it } from "vitest"
import { bidiControls } from "./bidi-controls.mjs"
import { shownCommand, shownJson, shownName } from "./said"

const spoof = "\u202Emoc.live@bob\u202C"

describe("shownName", () => {
  it("shows each bidi control as U+FFFD", () => {
    expect(shownName("evil\u202Egnp.exe")).toBe("evil\uFFFDgnp.exe")
    expect(shownName("a\u2069\u202Eb")).toBe("a\uFFFD\uFFFDb")
    expect(shownName("x\u200Ey\u200Fz\u061C")).toBe("x\uFFFDy\uFFFDz\uFFFD")
    expect(shownName("a\u2028b\u2029")).toBe("a\uFFFDb\uFFFD")
    expect(shownName("a\u0085b")).toBe("a\uFFFDb")
    expect(shownName("cargo")).toBe("cargo")
  })
})

describe("shownJson", () => {
  it("escapes a bidi control inside a string so the shown JSON parses to the same value", () => {
    const json = JSON.stringify({ to: spoof })
    const shown = shownJson(json)
    expect(shown).not.toMatch(bidiControls)
    expect(shown).toContain("\\u202e")
    expect(shown.indexOf("moc.live@bob")).toBeGreaterThan(shown.indexOf("\\u202e"))
    expect(JSON.parse(shown)).toEqual({ to: spoof })
  })

  it("leaves a control that is already a \\u escape as that escape", () => {
    const json = '{"to":"\\u202Emoc.live@bob\\u202C"}'
    expect(shownJson(json)).toBe(json)
    expect(JSON.parse(shownJson(json))).toEqual({ to: spoof })
  })

  it("does not treat a backslash before a control as ending the string", () => {
    const json = JSON.stringify({ to: `\\" ${spoof}` })
    expect(JSON.parse(shownJson(json))).toEqual(JSON.parse(json))
    expect(shownJson(json)).not.toMatch(bidiControls)
  })

  it("replaces a bidi control outside a string, which is not a value", () => {
    expect(shownJson("[\u202E]")).toBe("[\uFFFD]")
  })

  it("escapes a line or paragraph separator inside a string", () => {
    const value = "line\u2028next\u2029end\u0085"
    const json = JSON.stringify({ to: value })
    const shown = shownJson(json)
    expect(shown).not.toMatch(bidiControls)
    expect(shown).toContain("\\u2028")
    expect(shown).toContain("\\u2029")
    expect(shown).toContain("\\u0085")
    expect(JSON.parse(shown)).toEqual({ to: value })
  })
})

describe("shownCommand", () => {
  it("shows a tool's JSON argument with the control escaped, and the tool name with U+FFFD", () => {
    const command = `send\u202E ${JSON.stringify({ to: spoof })}`
    const shown = shownCommand(command)
    expect(shown.startsWith("send\uFFFD ")).toBe(true)
    expect(shown).not.toMatch(bidiControls)
    const argument = shown.slice(shown.indexOf(" ") + 1)
    expect(JSON.parse(argument)).toEqual({ to: spoof })
  })

  it("shows a command that is not JSON with each control as its escape, letters in order", () => {
    const command = `echo ${spoof}`
    const shown = shownCommand(command)
    expect(shown).toBe("echo \\u202emoc.live@bob\\u202c")
    expect(shown).not.toMatch(bidiControls)
    expect(shown.indexOf("moc.live@bob")).toBeGreaterThan(shown.indexOf("\\u202e"))
  })

  it("leaves a command with no bidi control as it runs", () => {
    const command = "cargo run -p nessa-gateway -- --simulate-clients 200"
    expect(shownCommand(command)).toBe(command)
  })
})
