/**
 * The panel for one conversation: the agents it put to work, and one
 * child's conversation with no composer. The hook decides what is open
 * (`use-subagents-panel.ts`); this draws it.
 */
import { AvatarStack } from "@nessa-ui/react/avatar-stack"
import { Breadcrumb } from "@nessa-ui/react/breadcrumb"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { Meter } from "@nessa-ui/react/meter"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import { StatusLabel } from "@nessa-ui/react/status-label"
import type { WidgetHost } from "../../widgets/ui/plugin"
import { elapsed, sessionTime, ReadOnlyTranscript } from "../../workspace"
import { plural } from "../../model/counts"
import { progressFraction, rowStatus, type Subagent } from "../model/subagent"
import { tagline } from "../model/tagline"
import { useSubagentsPanel, type SubagentsPanelModel } from "./use-subagents-panel"
import "./subagents.css"

export function SubagentsPanel({
  sessionId,
  host,
}: {
  sessionId: string
  host: WidgetHost
}) {
  const model = useSubagentsPanel(sessionId, host)
  return <SubagentsView model={model} />
}

export function SubagentsView({ model }: { model: SubagentsPanelModel }) {
  const { read, present } = model
  return (
    <section ref={model.rootRef} className="subagents-panel" data-subagent-panel>
      {present ? (
        <SubagentDetail model={model} subagent={present} />
      ) : read.kind === "failed" ? (
        <p className="subagents-failed" data-subagent-failed>
          Subagents could not be read.
        </p>
      ) : read.kind === "ready" &&
        read.subagents.length === 0 &&
        read.unreadable.length === 0 ? (
        <EmptyState
          data-subagent-empty
          title="No subagents"
          description="This conversation has not put any agents to work."
        />
      ) : read.kind === "ready" && read.subagents.length === 0 ? (
        <Unreadable keys={read.unreadable} />
      ) : read.kind === "ready" ? (
        <SubagentList model={model} unreadable={read.unreadable} />
      ) : null}
    </section>
  )
}

function SubagentList({
  model,
  unreadable,
}: {
  model: SubagentsPanelModel
  unreadable: readonly string[]
}) {
  return (
    <div className="subagents-list">
      <div className="subagents-heading">
        <AvatarStack
          max={Infinity}
          label={plural(model.ordered.length, "agent")}
          items={model.ordered.map((subagent) => ({
            seed: subagent.seed,
            name:
              subagent.lifecycle === "open" && subagent.activity === "working"
                ? `${subagent.name}, working`
                : subagent.name,
            busy: subagent.lifecycle === "open" && subagent.activity === "working",
          }))}
        />
      </div>
      <p className="subagents-summary" data-subagent-summary>
        {model.summary}
      </p>
      <div className="subagents-scroll">
        <ul className="subagents-rows" data-subagent-list>
          {model.ordered.map((subagent) => (
            <li key={subagent.id}>
              <SubagentRow
                subagent={subagent}
                now={model.now}
                onChoose={() => model.choose(subagent.id)}
              />
            </li>
          ))}
        </ul>
        {unreadable.length > 0 ? <Unreadable keys={unreadable} /> : null}
      </div>
    </div>
  )
}

function SubagentRow({
  subagent,
  now,
  onChoose,
}: {
  subagent: Subagent
  now: number
  onChoose: () => void
}) {
  const status = rowStatus(subagent)
  const fraction = progressFraction(subagent.progress)
  const live = subagent.conversation.activity
  return (
    <button
      type="button"
      className="subagent-row"
      data-subagent-row
      data-subagent-name={subagent.name}
      onClick={onChoose}
    >
      <RandomAvatar className="subagent-avatar" seed={subagent.seed} />
      <span className="subagent-name">
        {subagent.name}
        {subagent.tags.map((tag) => (
          <span key={tag.id} className="subagent-tag" data-hue={tag.hue}>
            {tag.name}
          </span>
        ))}
      </span>
      {subagent.model ? (
        <span className="subagent-model">{subagent.model.modelId}</span>
      ) : null}
      <span className="subagent-status">
        <StatusLabel tone={status.tone}>{status.word}</StatusLabel>
        <span className="subagent-time">{sessionTime(subagent.since, now)}</span>
        {subagent.progress ? (
          <Meter
            className="subagent-meter"
            label={`${subagent.name} progress`}
            value={fraction}
            valueText={`${subagent.progress.done} of ${subagent.progress.total}`}
          />
        ) : null}
      </span>
      <p className="subagent-headline">{subagent.headline}</p>
      {live ? (
        <p className="subagent-live" data-subagent-live>
          <span>{live.label}</span>
          <span>{elapsed(live.since, now)}</span>
        </p>
      ) : null}
      <p className="subagent-tagline">{tagline(subagent.seed)}</p>
    </button>
  )
}

function SubagentDetail({
  model,
  subagent,
}: {
  model: SubagentsPanelModel
  subagent: Subagent
}) {
  return (
    <div className="subagents-detail" data-subagent-detail>
      <Breadcrumb
        label="Subagent"
        back
        items={[
          { label: "Subagents", onSelect: () => model.choose(null) },
          { label: subagent.name },
        ]}
      />
      <div
        className="subagents-scroll"
        data-subagent-scroll
        ref={model.stick.ref}
        onScroll={model.stick.onScroll}
      >
        <div data-subagent-messages>
          <ReadOnlyTranscript
            messages={subagent.conversation.messages}
            activity={subagent.conversation.activity}
          />
        </div>
      </div>
    </div>
  )
}

function Unreadable({ keys }: { keys: readonly string[] }) {
  return (
    <p className="subagents-unreadable" data-subagent-unreadable>
      Could not read {keys.join(", ")}.
    </p>
  )
}
