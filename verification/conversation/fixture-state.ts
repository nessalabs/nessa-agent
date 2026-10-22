/** Published fixture inputs; authority remains the server projection's contract. */
import type { ConversationView as PublishedConversationView } from "@nessa/client"
import {
  conversationPermission,
  conversationQuestion,
  conversationView as decodeView,
} from "../../packages/nessa-client/src/protocol/conversation-validate"
import { conversationView as panelView } from "../../src/conversation/adapters/gateway/effects"
import type { ConversationView } from "../../src/conversation/application/view"
import type { ConversationTranscriptState } from "../../src/conversation/model"

export const states: ConversationTranscriptState[] = [
  "not_loaded",
  "partial",
  "stale",
  "unknown",
  "complete_empty",
  "complete",
]
// The review the live owner holds, as `permissionReview` reads it whole.
const COMMITTED_REVIEW_KEY = "0123456789abcdef0123456789abcdef"
export const committedReview = conversationPermission({
  review: COMMITTED_REVIEW_KEY,
  executionId: "exact-live-execution",
  permissionId: "committed-review",
  toolId: "tool",
  toolName: "Read",
  origin: { kind: "harness" },
  ask: "tool",
  title: "Read committed source",
  argumentsJson: '{"path":"src/main.rs"}',
  options: [{ id: "allow", label: "Allow once", effect: "allow" }],
})
export function decodePanelView(value: unknown, id: string): ConversationView {
  return projected(decodeView(value, id))
}
export function publishedView(
  state: ConversationTranscriptState,
  liveAuthority: boolean,
): PublishedConversationView {
  const actionable = state === "complete" && liveAuthority
  return decodeView(
    {
      conversationId: "committed-conversation",
      revision: `${state}:${liveAuthority}`,
      serverNowMs: 1_700_000_000_000,
      title: "Committed transcript",
      approvalMode: "ask",
      model: "claude-sonnet-5",
      approvalModes: [
        { id: "ask", name: "Provider asks", description: "Provider permission review." },
      ],
      transcriptState: state,
      queueComplete: state === "complete" || state === "complete_empty",
      truncated: false,
      messages:
        state === "complete"
          ? [
              {
                executionId: "exact-live-execution",
                userText: "Read committed source",
                attachments: [],
                files: [],
                status: liveAuthority ? "running" : "unresolved",
                parts: [],
              },
            ]
          : [],
      pending: [],
      artifacts: [],
      notes: [],
      unreadable: [],
      unreadableTruncated: false,
      handoffs: [],
      questions: [],
      tools: [],
      permissions: actionable
        ? [{ review: COMMITTED_REVIEW_KEY, origin: "harness", toolName: "Read" }]
        : [],
      capabilities: {
        queue: actionable,
        steer: actionable,
        resume: false,
        permissions: actionable,
        imageInput: false,
        agentFeatures: {
          permissionDenial: "unknown",
          nativeHookSuppression: "unknown",
          nativeHooks: "off",
          compactionReporting: "unsupported_not_implemented",
          modelSwitchReporting: "unsupported_not_implemented",
          permissionDeferral: "unsupported_not_implemented",
          elicitationForwarding: "unknown",
          preToolPolicy: "unsupported_not_implemented",
          policyEndTurn: "unsupported",
          policyCloseSession: "unsupported",
          incomingElicitation: "unsupported",
        },
      },
      lifecycle: { phase: liveAuthority ? "attached" : "absent" },
    } satisfies PublishedConversationView,
    "committed-conversation",
  )
}
export function view(
  state: ConversationTranscriptState,
  liveAuthority: boolean,
): ConversationView {
  const published = publishedView(state, liveAuthority)
  return projected(published)
}
export const committedQuestion = conversationQuestion({
  question: "fedcba9876543210fedcba9876543210",
  executionId: "exact-live-execution",
  questionId: "retained-question",
  message: "Choose the environments",
  questions: [0, 1, 2].map((index) => ({
    key: `environment_${index}`,
    prompt: `Environment ${index}?`,
    multiSelect: false,
    freeText: false,
    required: false,
    options: [{ value: "staging", label: "Staging" }],
  })),
})

/** A fixture supplies only the detail held under the exact listed key. */
function projected(published: PublishedConversationView): ConversationView {
  return panelView(
    published,
    1_700_000_000_000,
    published.permissions.flatMap((entry) =>
      entry.review === committedReview.review
        ? [{ state: "loaded" as const, entry, shown: committedReview }]
        : [],
    ),
    published.questions.flatMap((entry) =>
      entry.question === committedQuestion.question
        ? [{ state: "loaded" as const, entry, shown: committedQuestion }]
        : [],
    ),
  )
}

export function publishedQuestionView(limited: boolean): PublishedConversationView {
  return decodeView(
    {
      ...publishedView("complete", true),
      revision: `questions:${limited}`,
      truncated: limited,
      interactionViewError: limited
        ? "Some pending interactions exceed this display limit. Use Stop to cancel them."
        : undefined,
      permissions: [],
      questions: [
        { question: committedQuestion.question, message: committedQuestion.message },
      ],
    },
    "committed-conversation",
  )
}
export function questionView(limited: boolean): ConversationView {
  return projected(publishedQuestionView(limited))
}
