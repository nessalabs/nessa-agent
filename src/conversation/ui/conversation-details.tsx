import { useId, useState, type ReactElement, type ReactNode } from "react"
import { conversationLeasePlace, conversationLeaseStatus } from "@nessa/client"
import { Info, Pencil } from "lucide-react"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@nessa-ui/react/context-menu"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetExpand,
  SheetTitle,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
// Setup owns what an agent is called and how its mark is drawn. These two
// modules are imported directly, not through the onboarding barrel, which also
// carries the setup gate and with it the host.
import { AGENT_CHOICES } from "../../onboarding/model/onboarding"
import { AgentMark } from "../../onboarding/ui/agent-mark"
import type { AgentFeatures, Conversation, ConversationLease } from "../model"

type FeatureSupport = AgentFeatures[keyof AgentFeatures]

/** One capability in a word or two: whether it works here, not how. */
function support(value: FeatureSupport) {
  switch (value) {
    case "unknown":
      return "Not verified"
    case "unsupported":
      return "Not available"
    case "unsupported_not_implemented":
      return "Not yet in Nessa"
    case "supported_for_offered_permission_reviews":
    case "supported_for_user_configured_hooks":
    case "supported_with_invocation_correlation":
    case "supported_after_validated_switch":
    case "supported_with_nonterminal_outcome":
    case "supported_with_correlated_round_trip":
    case "supported_at_permission_gate":
    case "supported_for_current_invocation":
    case "supported_for_session":
      return "Supported"
    default: {
      const exhaustive: never = value
      return exhaustive
    }
  }
}

const tokenCount = new Intl.NumberFormat("en", { maximumFractionDigits: 1 })

/** A context window in words: "1 million tokens", "200,000 tokens". */
function tokens(count: number) {
  return count >= 1_000_000 && count % 100_000 === 0
    ? `${tokenCount.format(count / 1_000_000)} million tokens`
    : `${tokenCount.format(count)} tokens`
}

/** A group of facts on a soft fill. Space separates the rows, not rules. */
function FactGroup({ title, children }: { title: string; children: ReactNode }) {
  // An h3 under the sheet's own h2 title, and the section named by it.
  const heading = useId()
  return (
    <section aria-labelledby={heading} className="flex flex-col gap-2">
      <h3
        id={heading}
        className="m-0 px-4 nessa-text-2 font-medium text-muted-foreground"
      >
        {title}
      </h3>
      <div className="flex flex-col rounded-[18px] bg-muted/60 px-4 py-1.5">
        {children}
      </div>
    </section>
  )
}

/** A label and its value. A long value wraps rather than being cut short: a
 * fact read only in part is not read (a refused lease's reason, at the
 * panel's narrowest). */
function Fact({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-2.5">
      <span className="shrink-0 nessa-text-4 text-foreground">{label}</span>
      <span className="min-w-0 break-words text-end nessa-text-4 text-muted-foreground">
        {value}
      </span>
    </div>
  )
}

/**
 * What a conversation runs on and how its tools are approved. Read-only: the
 * composer is where the model and the approval mode are chosen.
 */
function ConversationFacts({ conversation }: { conversation: Conversation }) {
  const remote = conversation.remote
  const runtime = remote?.runtime
  const features = remote?.capabilities.agentFeatures
  // Narrowed by asking setup's list, which owns what an agent is called.
  const agent = AGENT_CHOICES.find((choice) => choice.id === runtime?.agent)
  return (
    <div className="flex flex-col gap-7 pb-4">
      {runtime ? (
        <div className="flex flex-col items-center gap-1 pt-2 text-center">
          <p className="m-0 inline-flex min-w-0 items-center gap-2 nessa-text-7 font-semibold tracking-tight text-foreground">
            {agent ? <AgentMark id={agent.id} name={agent.name} /> : null}
            <span className="min-w-0 truncate">{runtime.modelName}</span>
          </p>
          <p className="m-0 nessa-text-3 text-muted-foreground">
            {agent?.name ?? runtime.provider}
          </p>
        </div>
      ) : (
        <p className="m-0 pt-2 text-center nessa-text-3 text-muted-foreground">
          Details appear once the agent has started.
        </p>
      )}
      {remote ? (
        <FactGroup title="Model">
          {runtime ? (
            <Fact label="Model max context" value={tokens(runtime.contextWindowTokens)} />
          ) : null}
          {runtime ? (
            <Fact label="Reasoning" value={runtime.reasoning ? "On" : "Off"} />
          ) : null}
          <Fact
            label="Images"
            value={remote.capabilities.imageInput ? "Supported" : "Not supported"}
          />
        </FactGroup>
      ) : null}
      {remote ? (
        <FactGroup title="Approvals">
          <Fact
            label="Tool calls"
            value={
              remote.approvalModes.find((choice) => choice.id === remote.approvalMode)
                ?.name ?? remote.approvalMode
            }
          />
          {features ? (
            <>
              <Fact label="Deny a request" value={support(features.permissionDenial)} />
              <Fact label="Answer later" value={support(features.permissionDeferral)} />
            </>
          ) : (
            <Fact label="Deny a request" value="Not verified" />
          )}
        </FactGroup>
      ) : null}
      {remote?.lease ? <LeaseFacts lease={remote.lease} /> : null}
      {runtime ? (
        <FactGroup title="Workspace">
          <div className="py-2.5 nessa-text-4 break-all text-foreground">
            {runtime.workspace}
          </div>
        </FactGroup>
      ) : null}
    </div>
  )
}

/** Where the agent runs and the sandbox around it, from its latest lease. */
function LeaseFacts({ lease }: { lease: ConversationLease }) {
  // A refused lease names what was asked for, not where anything runs —
  // except the SSH host it was asked of, which is what a person fixes.
  const granted = lease.state !== "refused"
  const place = conversationLeasePlace(lease) ?? "Not known"
  return (
    <FactGroup title="Where it runs">
      {!granted && lease.environment === "ssh" ? (
        <Fact label="SSH host" value={place} />
      ) : null}
      {granted ? (
        <>
          <Fact
            label={lease.environment === "ssh" ? "SSH host" : "Computer"}
            value={place}
          />
          <Fact
            label="Sandbox"
            value={lease.sandbox === "harness_default" ? "The agent's own" : "Not known"}
          />
        </>
      ) : null}
      <Fact label="Status" value={conversationLeaseStatus(lease)} />
    </FactGroup>
  )
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
          <ConversationFacts conversation={conversation} />
        )}
      </SheetBody>
    </Sheet>
  )
}
