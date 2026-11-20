/**
 * Chat context: the right panel's first tab, for the chat in front. What it
 * runs on (Environment), the model and its window (Context), and the files
 * its agent published (Outputs), each a section that folds from its header.
 * It only reads the chat's transcript; nothing here changes the chat.
 */
import { useId, type ReactNode } from "react"
import { conversationLeasePlace } from "@nessa/client"
import { GroupHeader } from "@nessa-ui/react/group-header"
import { formatFileSize } from "@nessa-ui/react/file-preview"
import { SavePublishedFile } from "../../../../conversation"
import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
import { toggleChatContextSection } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChatContextFolded, selectTranscript } from "../../adapters/store/selectors"
import type { Transcript } from "../../model/transcript"
import { useNativePlugins } from "../../../widgets"
import { useAccessoryOpen } from "../../adapters/store/widget-hosts"
import "./chat-context.css"

type SectionId = "environment" | "context" | "outputs"

export function ChatContext({ sessionId }: { sessionId: string }) {
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  // One set for every chat, so a section folded in one stays folded in the next.
  const shut = useWorkspaceSelector(selectChatContextFolded)
  const dispatch = useWorkspaceDispatch()
  const toggle = (id: SectionId) => dispatch(toggleChatContextSection({ sectionId: id }))
  const published = transcript?.published ?? []
  return (
    <section className="chat-context" aria-label="Chat context" data-chat-context>
      <Section id="environment" label="Environment" shut={shut} onToggle={toggle}>
        <Environment transcript={transcript} />
      </Section>
      <Section id="context" label="Context" shut={shut} onToggle={toggle}>
        <ContextFacts transcript={transcript} />
        <PluginRows sessionId={sessionId} />
      </Section>
      <Section
        id="outputs"
        label="Outputs"
        count={published.length || undefined}
        shut={shut}
        onToggle={toggle}
      >
        {published.length === 0 ? (
          <p className="chat-context-none">Nothing published yet</p>
        ) : (
          <ul className="chat-context-rows" aria-label="Published files">
            {published.map((file, index) => (
              // The same file may be published twice; its place tells the two apart.
              <li key={`${file.digest}:${index}`} className="chat-context-row">
                <DesktopIcon name="file" size={16} aria-hidden="true" />
                <span className="chat-context-name" title={file.name}>
                  {file.name}
                </span>
                <span className="chat-context-trail">{formatFileSize(file.size)}</span>
                <SavePublishedFile conversationId={sessionId} file={file} />
              </li>
            ))}
          </ul>
        )}
      </Section>
    </section>
  )
}

function Section({
  id,
  label,
  count,
  shut,
  onToggle,
  children,
}: {
  id: SectionId
  label: string
  count?: number
  shut: readonly string[]
  onToggle: (id: SectionId) => void
  children: ReactNode
}) {
  const body = useId()
  const open = !shut.includes(id)
  return (
    <div className="chat-context-section" data-chat-context-section={id} data-open={open}>
      <GroupHeader
        level={3}
        size="dense"
        tone="quiet"
        label={label}
        count={count}
        open={open}
        onOpenChange={() => onToggle(id)}
        controls={body}
      />
      {open ? (
        <div id={body} className="chat-context-body">
          {children}
        </div>
      ) : null}
    </div>
  )
}

function Row({
  icon,
  label,
  trail,
  title,
}: {
  icon: DesktopIconRole
  label: string
  trail?: string
  title?: string
}) {
  return (
    <div className="chat-context-row" title={title}>
      <DesktopIcon name={icon} size={16} aria-hidden="true" />
      <span className="chat-context-name">{label}</span>
      {trail ? <span className="chat-context-trail">{trail}</span> : null}
    </div>
  )
}

/**
 * What each widget plugin has for this chat, such as its subagents (ADR 326's
 * `SessionAccessory`): one row each, named by the plugin, holding the
 * plugin's own control, which opens its tab for this chat in the panel. A
 * plugin with nothing for this chat draws nothing, and its row is not shown
 * (`chat-context.css`).
 */
function PluginRows({ sessionId }: { sessionId: string }) {
  const plugins = useNativePlugins()
  const open = useAccessoryOpen(sessionId)
  return plugins.map(({ id, name, SessionAccessory }) =>
    SessionAccessory ? (
      <div key={id} className="chat-context-row chat-context-plugin" data-plugin={id}>
        <span className="chat-context-name">{name}</span>
        <span className="chat-context-plugin-accessory">
          <SessionAccessory sessionId={sessionId} openWidget={open} />
        </span>
      </div>
    ) : null,
  )
}

/** Where the agent runs, and the folder it works in there. */
function Environment({ transcript }: { transcript: Transcript | undefined }) {
  const lease = transcript?.lease
  const place =
    lease && lease.state !== "unreadable" ? conversationLeasePlace(lease) : undefined
  const workspace = transcript?.runtime?.workspace
  if (!place && !workspace) return <p className="chat-context-none">Not started yet</p>
  return (
    <>
      {place ? <Row icon="device" label={place} /> : null}
      {workspace ? (
        <Row
          icon="folder"
          label={folderName(workspace)}
          trail={workspace}
          title={workspace}
        />
      ) : null}
    </>
  )
}

/** The model the chat runs, and how much its catalog says it can hold. */
function ContextFacts({ transcript }: { transcript: Transcript | undefined }) {
  const runtime = transcript?.runtime
  if (!runtime) return <p className="chat-context-none">Not started yet</p>
  return (
    <Row
      icon="model"
      label={runtime.modelName}
      trail={`${tokens(runtime.contextWindowTokens)} context`}
    />
  )
}

/** The last part of a path, which is what people call a folder by. */
export function folderName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean)
  return parts[parts.length - 1] ?? path
}

/** A token count as people say it: 200K, 1M. */
export function tokens(count: number): string {
  if (count >= 1_000_000) return `${trim(count / 1_000_000)}M`
  if (count >= 1_000) return `${trim(count / 1_000)}K`
  return String(count)
}

function trim(value: number): string {
  return value >= 10 ? String(Math.round(value)) : value.toFixed(1).replace(/\.0$/, "")
}
