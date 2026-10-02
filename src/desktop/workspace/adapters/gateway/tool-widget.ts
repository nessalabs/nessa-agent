/**
 * A gateway conversation's tool call, read as the transcript's widget part,
 * named in that conversation (`conversationId`).
 *
 * The gateway names a call's MCP server and tool, and — from its own
 * connection to that server, since no harness passes it (ADR 344, #346) — the
 * UI resource the tool declared. Whether that makes a widget is the model's
 * rule (`toolWidget`); this only reads the wire's shape into it. The desktop's
 * gateway source (#248) calls it for each tool of a conversation view; until
 * that source exists, `scripts/mcp-test-server/live-check.mjs` applies it to
 * a real view.
 */
import type { ConversationTool } from "@nessa/client"
import { toolWidget, type Part } from "../../model/transcript"

export function gatewayToolWidget(
  conversationId: string,
  tool: ConversationTool,
): Extract<Part, { kind: "widget" }> | null {
  return toolWidget({
    sessionId: conversationId,
    executionId: tool.executionId,
    toolId: tool.toolId,
    ...(tool.mcp ? { mcp: tool.mcp } : {}),
  })
}
