import { describe, expect, it } from "vitest"
import { folderName, publishedKeys, tokens } from "./chat-context"

describe("Chat context's words", () => {
  it("names a folder by the last part of its path", () => {
    expect(folderName("/Users/me/Documents/NessaLabs/nessa-agent")).toBe("nessa-agent")
    expect(folderName("C:\\\\code\\\\nessa-agent\\\\")).toBe("nessa-agent")
    expect(folderName("/")).toBe("/")
  })

  it("says a token count as people do", () => {
    expect(tokens(950)).toBe("950")
    expect(tokens(200_000)).toBe("200K")
    expect(tokens(1_000_000)).toBe("1M")
    expect(tokens(1_500_000)).toBe("1.5M")
  })

  it("keys each published file by what came before it, so a newer one moves no key", () => {
    const plan = { name: "plan.md", digest: "sha256:a" }
    const shot = { name: "shot.png", digest: "sha256:b" }
    const before = publishedKeys([plan, shot]).map(({ key }) => key)
    // A new copy of the plan arrives first in the newest-first list.
    const after = publishedKeys([plan, plan, shot]).map(({ key }) => key)
    expect(after.slice(1)).toEqual(before)
    expect(new Set(after).size).toBe(3)
  })
})
