import { expect, it, vi } from "vitest"
import { escapeStack } from "./escape-stack"

it("runs the step back registered last, and only that", () => {
  const steps = escapeStack()
  const first = vi.fn()
  const second = vi.fn()
  steps.push(first)
  const removeSecond = steps.push(second)
  expect(steps.escape()).toBe(true)
  expect(second).toHaveBeenCalledTimes(1)
  expect(first).not.toHaveBeenCalled()
  removeSecond()
  expect(steps.escape()).toBe(true)
  expect(first).toHaveBeenCalledTimes(1)
})

it("answers false with nothing registered, so the host's own Escape follows", () => {
  const steps = escapeStack()
  expect(steps.escape()).toBe(false)
  const remove = steps.push(() => {})
  remove()
  // Removed twice, it takes nothing else with it.
  const kept = vi.fn()
  steps.push(kept)
  remove()
  expect(steps.escape()).toBe(true)
  expect(kept).toHaveBeenCalledTimes(1)
})

it("holds one handler registered twice as two", () => {
  const steps = escapeStack()
  const step = vi.fn()
  const removeOne = steps.push(step)
  steps.push(step)
  removeOne()
  expect(steps.escape()).toBe(true)
  expect(step).toHaveBeenCalledTimes(1)
})
