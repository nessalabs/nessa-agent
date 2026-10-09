/** The production details sheet over published gateway views, one per lease state. */
import { createRoot } from "react-dom/client"
import { conversationView as decodeView } from "../../packages/nessa-client/src/protocol/conversation-validate"
import { applyView } from "../../src/conversation/application/usecases/apply-view"
import { conversation, type Conversation } from "../../src/conversation/model"
import { ConversationDetails } from "../../src/conversation/ui/conversation-details"
import "../../src/styles.css"

const terms = { revision: 2, environment: "here", sandbox: "harness_default" }

/** Each case is a lease `lease_view` can produce, by its state. */
const leaseCases: Record<string, Record<string, unknown> | undefined> = {
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
}

function withLease(lease: Record<string, unknown> | undefined): Conversation {
  const id = "lease-conversation"
  const view = decodeView(
    {
      conversationId: id,
      revision: `lease:${lease?.state ?? "none"}`,
      title: "Leased conversation",
      approvalMode: "ask",
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
      capabilities: {
        queue: true,
        steer: true,
        resume: false,
        permissions: true,
        imageInput: false,
        agentFeatures: {
          permissionDenial: "unknown",
          nativeHookSuppression: "unknown",
          compactionReporting: "unsupported_not_implemented",
          modelSwitchReporting: "unsupported_not_implemented",
          permissionDeferral: "unsupported_not_implemented",
          elicitationForwarding: "unknown",
          preToolPolicy: "unsupported_not_implemented",
          policyEndTurn: "unsupported_not_implemented",
          policyCloseSession: "unsupported_not_implemented",
          incomingElicitation: "unknown",
        },
      },
      lifecycle: { phase: "attached" },
      ...(lease ? { lease } : {}),
    },
    id,
  )
  return applyView(conversation(id), view)
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
        conversation={withLease(leaseCases[name])}
        rename={false}
        onClose={() => {}}
        onRename={() => {}}
      />
    </main>
  )
}

createRoot(document.getElementById("root")!).render(<Fixture />)
