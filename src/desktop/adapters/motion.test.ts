// @vitest-environment jsdom
import { describe, expect, it } from "vitest"
import { curveToken } from "./motion"

const element = (curve: string) => {
  const node = document.createElement("div")
  node.style.setProperty("--curve", curve)
  document.body.append(node)
  return node
}

describe("a curve token in script", () => {
  it("follows the stylesheet's cubic-bezier, from 0 to 1", () => {
    const ease = curveToken(element("cubic-bezier(0.22, 1, 0.36, 1)"), "--curve")
    expect(ease(0)).toBe(0)
    expect(ease(1)).toBe(1)
    // An ease-out: well past halfway at the halfway mark, and rising throughout.
    expect(ease(0.5)).toBeGreaterThan(0.85)
    const samples = Array.from({ length: 11 }, (_, step) => ease(step / 10))
    expect(samples).toEqual([...samples].sort((a, b) => a - b))
  })

  it("matches a known point of the standard ease", () => {
    // CSS `ease` is cubic-bezier(0.25, 0.1, 0.25, 1); at x = 0.5 its y is about 0.8024.
    const ease = curveToken(element("cubic-bezier(0.25, 0.1, 0.25, 1)"), "--curve")
    expect(ease(0.5)).toBeCloseTo(0.8024, 3)
  })

  it("is linear when the token is not a cubic-bezier, or unset", () => {
    expect(curveToken(element("ease"), "--curve")(0.3)).toBe(0.3)
    expect(curveToken(element(""), "--missing")(0.3)).toBe(0.3)
  })
})
