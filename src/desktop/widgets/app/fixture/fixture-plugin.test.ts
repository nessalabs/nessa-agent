/**
 * The fixture app's conversation (#390, D-J): a message is held to the
 * client's bounds as the gateway's adapter holds it, so the sample draws no
 * message the gateway would refuse (gate 7).
 */
import { MAX_MCP_MESSAGE_BYTES, mcpAppRequestProblem } from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import type { AppAddress } from "../application/ports"
import { fixtureConversation } from "./fixture-plugin"
import { fixtureServer } from "./fixture-widgets"

const address: AppAddress = {
  sessionId: "sample",
  server: fixtureServer,
  app: { executionId: "e", toolId: "t", instanceId: "mount" },
}
const text = (value: string) => ({ type: "text", text: value })

describe("the fixture app's conversation", () => {
  it("D3: refuses a message the client's bounds refuse, in its words, and writes nothing", async () => {
    const write = vi.fn(async () => {})
    const conversation = fixtureConversation(write)
    for (const [content, joined] of [
      [[], ""],
      [[text("")], ""],
      [
        [text("x".repeat(MAX_MCP_MESSAGE_BYTES + 1))],
        "x".repeat(MAX_MCP_MESSAGE_BYTES + 1),
      ],
      [[text("\uD800")], "\uD800"],
    ] as const)
      expect(await conversation.sendMessage(address, content)).toEqual({
        kind: "invalid",
        reason: mcpAppRequestProblem.message(joined),
      })
    expect(write).not.toHaveBeenCalled()
  })

  it("D1: writes a message within them as the fixture server's app, as the gateway's adapter joins it", async () => {
    const write = vi.fn(async () => {})
    expect(
      await fixtureConversation(write).sendMessage(address, [
        text("Plot May"),
        text("next to April"),
      ]),
    ).toEqual({ kind: "ok", result: {} })
    expect(write).toHaveBeenCalledExactlyOnceWith(
      "sample",
      { server: fixtureServer, tool: "show_fixture" },
      "Plot May\n\nnext to April",
    )
  })
})
