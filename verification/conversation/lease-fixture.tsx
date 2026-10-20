/** The production details sheet over published gateway views, one per lease state. */
import type { ConversationLease, ConversationView } from "@nessa/client"
import { createRoot } from "react-dom/client"
import { conversationView as decodeView } from "../../packages/nessa-client/src/protocol/conversation-validate"
import { conversationView as panelView } from "../../src/conversation/adapters/gateway/effects"
import { applyView } from "../../src/conversation/application/usecases/apply-view"
import { conversation, type Conversation } from "../../src/conversation/model"
import { ConversationDetails } from "../../src/conversation/ui/conversation-details"
import "../../src/styles.css"

const terms = { revision: 2, environment: "here", sandbox: "harness_default" } as const

/** Each case is a lease `lease_view` can produce, by its state. */
const leaseCases: Record<string, ConversationLease | undefined> = {
  none: undefined,
  live: { state: "live", ...terms, droppedEvents: 0 },
  ending: { state: "ending", ...terms, cause: "closed", droppedEvents: 0 },
  ended: {
    state: "ended",
    ...terms,
    cause: "lost",
    cleanup: "not_held",
    droppedEvents: 2,
  },
  interrupted: { state: "interrupted", ...terms, cause: "stopped", droppedEvents: 0 },
  refused: {
    state: "refused",
    ...terms,
    refusal: "sandbox_unavailable",
    droppedEvents: 0,
  },
  unreadable: { state: "unreadable", droppedEvents: 0 },
  // On an SSH host the gateway names: the host is said wherever it runs.
  "ssh-live": {
    state: "live",
    ...terms,
    environment: "ssh",
    host: "me@build-01.internal.example.com",
    droppedEvents: 0,
  },
  "ssh-lost": {
    state: "ended",
    ...terms,
    environment: "ssh",
    host: "devbox",
    cause: "lost",
    cleanup: "confirmed",
    droppedEvents: 0,
  },
  "ssh-refused": {
    state: "refused",
    ...terms,
    environment: "ssh",
    host: "devbox",
    refusal: "environment_unreachable",
    droppedEvents: 0,
  },
  // On a paired machine that dials in: named by the start of its key.
  "peer-live": {
    state: "live",
    ...terms,
    environment: "peer",
    host: `peer:${"ab".repeat(32)}`,
    droppedEvents: 0,
  },
  "peer-lost": {
    state: "ended",
    ...terms,
    environment: "peer",
    host: `peer:${"0f".repeat(32)}`,
    cause: "lost",
    cleanup: "confirmed",
    droppedEvents: 0,
  },
  "peer-refused": {
    state: "refused",
    ...terms,
    environment: "peer",
    host: `peer:${"0f".repeat(32)}`,
    refusal: "environment_unreachable",
    droppedEvents: 0,
  },
}

function withLease(lease: ConversationLease | undefined): Conversation {
  const id = "lease-conversation"
  const view = decodeView(
    {
      conversationId: id,
      revision: `lease:${lease?.state ?? "none"}`,
      serverNowMs: 1_700_000_000_000,
      title: "Leased conversation",
      approvalMode: "ask",
      model: "claude-sonnet-5",
      approvalModes: [
        { id: "ask", name: "Provider asks", description: "Provider permission review." },
      ],
      transcriptState: "complete_empty",
      queueComplete: true,
      truncated: false,
      messages: [],
      pending: [],
      questions: [],
      tools: [],
      permissions: [],
      artifacts: [],
      notes: [],
      unreadable: [],
      unreadableTruncated: false,
      capabilities: {
        queue: true,
        steer: true,
        resume: false,
        permissions: true,
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
      lifecycle: { phase: "attached" },
      ...(lease ? { lease } : {}),
    } satisfies ConversationView,
    id,
  )
  return applyView(conversation(id), panelView(view, 1_700_000_000_000))
}

/** One case per page, named by `?case=`: the sheet is modal, so nothing
 * beside it is pressed to switch. */
function Fixture() {
  const name = new URLSearchParams(location.search).get("case") ?? "none"
  return (
    // The sheet fills the panel it opens over; here, the page.
    <main
      data-lease-fixture
      data-case={name}
      style={{ position: "relative", height: "100vh" }}
    >
      <ConversationDetails
        conversation={withLease(
          Object.hasOwn(leaseCases, name) ? leaseCases[name] : undefined,
        )}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
      />
    </main>
  )
}

createRoot(document.getElementById("root")!).render(<Fixture />)
