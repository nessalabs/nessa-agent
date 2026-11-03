import { describe, expect, it } from "vitest"
import { chosenEnvironment, HERE, linksFilesHere } from "./types"

const selection = (environment?: string) => ({
  agent: "claude",
  model: "sonnet",
  approvalMode: "ask" as const,
  ...(environment === undefined ? {} : { environment }),
})

describe("where a draft runs", () => {
  it("runs on the default device until a place is chosen", () => {
    expect(chosenEnvironment(selection(), "devbox")).toBe("devbox")
    expect(chosenEnvironment(undefined, "devbox")).toBe("devbox")
    expect(chosenEnvironment(selection(), undefined)).toBeUndefined()
  })

  it("keeps a chosen place, this computer included, over the default", () => {
    expect(chosenEnvironment(selection("build"), "devbox")).toBe("build")
    expect(chosenEnvironment(selection(HERE), "devbox")).toBeUndefined()
  })

  it("links files only where this computer is chosen or nothing is", () => {
    expect(linksFilesHere({ selection: selection(HERE) })).toBe(true)
    expect(linksFilesHere({ selection: selection() })).toBe(true)
    expect(linksFilesHere({ selection: selection("devbox") })).toBe(false)
  })
})
