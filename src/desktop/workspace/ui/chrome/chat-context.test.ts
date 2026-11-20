import { describe, expect, it } from "vitest"
import { folderName, tokens } from "./chat-context"

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
})
