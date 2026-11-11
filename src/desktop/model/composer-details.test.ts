import { expect, it } from "vitest"
import {
  defaultComposerDetails,
  otherComposerDetails,
  parseComposerDetails,
} from "./composer-details"

it("reads a stored choice and falls back for anything else", () => {
  expect(parseComposerDetails("hidden")).toBe("hidden")
  expect(parseComposerDetails("shown")).toBe("shown")
  expect(parseComposerDetails(null)).toBe(defaultComposerDetails)
  expect(parseComposerDetails("collapsed")).toBe(defaultComposerDetails)
  expect(parseComposerDetails(1)).toBe(defaultComposerDetails)
})

it("turns each choice into the other", () => {
  expect(otherComposerDetails("shown")).toBe("hidden")
  expect(otherComposerDetails("hidden")).toBe("shown")
})
