/**
 * The deadline fixture's gateway views, validated by the actual client contract.
 * Pending keeps its armed review; expiry publishes that permission's deadline
 * notice and removes the review. The page owns the unchanged ten-second timer.
 */
import type { ConversationPermission, ConversationView } from "@nessa/client"
import { conversationView } from "../../../../packages/nessa-client/src/protocol/conversation-validate"
import {
  type Unkeyed,
  view,
} from "../../../../src/desktop/workspace/adapters/gateway/fake-gateway"

export const conversation = "1c2b3a4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
export const reason =
  "No human answered in time. Continue without this tool or try another approach."
export const waitMs = 10_000

export function armed(dueAtMs: number): Unkeyed<ConversationPermission> {
  return {
    executionId: "run",
    permissionId: "1",
    toolId: "command-1",
    title: "Run the release tests",
    toolName: "bash",
    argumentsJson: '{"command":"cargo test"}',
    origin: { kind: "harness" },
    ask: "tool",
    options: [
      { id: "allow", label: "Allow", effect: "allow" },
      { id: "deny", label: "Deny", effect: "deny" },
    ],
    deadline: {
      waitMs,
      dueAtMs,
      effect: "deny",
      reason,
      stopsTurn: false,
    },
  }
}

export function shown(
  revision: number,
  review: Unkeyed<ConversationPermission> | undefined,
): ConversationView {
  return conversationView(
    view(conversation, {
      revision: String(revision),
      approvalModes: [
        { id: "ask", name: "Ask", description: "Asks before each tool it runs." },
      ],
      messages: [
        {
          executionId: "run",
          userText: "Sign the release.",
          attachments: [],
          files: [],
          status: review ? "running" : "completed",
          parts: review
            ? []
            : [
                {
                  offset: 0,
                  kind: "local_notice",
                  text: `Denied automatically: ${reason}`,
                  toolId: "",
                  noticeId: "1",
                  noticeKind: "deadline",
                },
              ],
        },
      ],
      permissions: review ? [review] : [],
    }),
    conversation,
  )
}
