import { useState, type ReactElement } from "react"
import { Info, Pencil } from "lucide-react"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@nessa-ui/react/context-menu"
import {
  AgentDetails,
  AgentDetailsSection,
  AgentDetailsField,
} from "@nessa-ui/react/agent-details"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetExpand,
  SheetTitle,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import type { AgentFeatures, Conversation } from "../model"

type FeatureSupport = AgentFeatures[keyof AgentFeatures]

function support(
  value: FeatureSupport,
  available: string,
  unavailable = "Unavailable",
  notImplemented = "Not implemented in Nessa",
) {
  switch (value) {
    case "unknown":
      return "Not verified"
    case "unsupported":
      return unavailable
    case "unsupported_not_implemented":
      return notImplemented
    case "supported_for_offered_permission_reviews":
    case "supported_for_user_configured_hooks":
    case "supported_with_invocation_correlation":
    case "supported_after_validated_switch":
    case "supported_with_nonterminal_outcome":
    case "supported_with_correlated_round_trip":
    case "supported_at_permission_gate":
    case "supported_for_current_invocation":
    case "supported_for_session":
      return available
    default: {
      const exhaustive: never = value
      return exhaustive
    }
  }
}

export function ConversationTabMenu({
  children,
  onDetails,
  onRename,
}: {
  children: ReactElement
  onDetails: () => void
  onRename: () => void
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={onDetails}>
          <Info />
          View details
        </ContextMenuItem>
        <ContextMenuItem onSelect={onRename}>
          <Pencil />
          Rename
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
export function ConversationDetails({
  conversation,
  rename,
  onClose,
  onRename,
}: {
  conversation: Conversation
  rename: boolean
  onClose: () => void
  onRename: (title: string) => void
}) {
  const [title, setTitle] = useState(conversation.title)
  const runtime = conversation.remote?.runtime
  const features = conversation.remote?.capabilities.agentFeatures
  return (
    <Sheet
      className="nessa-detail-sheet"
      label={rename ? "Rename conversation" : "Conversation details"}
      onClose={onClose}
    >
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{rename ? "Rename" : "Details"}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        {rename ? (
          <form
            onSubmit={(event) => {
              event.preventDefault()
              if (title.trim()) {
                onRename(title.trim())
                onClose()
              }
            }}
            className="flex flex-col gap-4"
          >
            <label className="text-sm">
              Conversation name
              <input
                aria-label="Conversation name"
                value={title}
                maxLength={120}
                onChange={(event) => setTitle(event.target.value)}
                className="mt-2 w-full rounded-lg border p-3"
              />
            </label>
            <button
              type="submit"
              disabled={!title.trim()}
              className="rounded-full bg-primary px-4 py-2 text-primary-foreground disabled:opacity-50"
            >
              Save
            </button>
          </form>
        ) : (
          <AgentDetails title={conversation.title}>
            <AgentDetailsSection title="Info">
              {runtime ? (
                <>
                  <AgentDetailsField label="Runtime" value={runtime.provider} />
                  <AgentDetailsField label="Model" value={runtime.model} />
                  <AgentDetailsField
                    label="Working directory"
                    value={<span className="break-all">{runtime.workspace}</span>}
                  />
                </>
              ) : (
                <p className="text-sm text-muted-foreground">
                  Runtime details are not available yet.
                </p>
              )}
            </AgentDetailsSection>
            <AgentDetailsSection title="Capabilities">
              {features ? (
                <>
                  <AgentDetailsField
                    label="Deny requested tool access"
                    value={support(
                      features.permissionDenial,
                      "Available when the agent offers a deny choice",
                    )}
                  />
                  <AgentDetailsField
                    label="Isolate user hooks"
                    value={support(
                      features.nativeHookSuppression,
                      "Verified for user-configured hooks",
                    )}
                  />
                  <AgentDetailsField
                    label="Report context compaction"
                    value={support(
                      features.compactionReporting,
                      "Available with turn correlation",
                    )}
                  />
                  <AgentDetailsField
                    label="Report model changes"
                    value={support(
                      features.modelSwitchReporting,
                      "Available after validation",
                    )}
                  />
                  <AgentDetailsField
                    label="Explicitly defer permission decisions"
                    value={support(
                      features.permissionDeferral,
                      "Available with a later correlated answer",
                      "Unavailable",
                      "Explicit later-answer outcomes are not implemented in Nessa",
                    )}
                  />
                  <AgentDetailsField
                    label="Provider question forwarding"
                    value={support(
                      features.elicitationForwarding,
                      "Available with a correlated reply",
                    )}
                  />
                  <AgentDetailsField
                    label="Apply policies before tools run"
                    value={support(
                      features.preToolPolicy,
                      "Available for tools awaiting permission",
                    )}
                  />
                  <AgentDetailsField
                    label="End a turn from policy"
                    value={support(features.policyEndTurn, "Available")}
                  />
                  <AgentDetailsField
                    label="Close a session from policy"
                    value={support(features.policyCloseSession, "Available")}
                  />
                  <AgentDetailsField
                    label="Answer incoming questions in Nessa"
                    value={support(features.incomingElicitation, "Available")}
                  />
                </>
              ) : (
                <p className="text-sm text-muted-foreground">
                  Capability details are not available yet.
                </p>
              )}
            </AgentDetailsSection>
          </AgentDetails>
        )}
      </SheetBody>
    </Sheet>
  )
}
