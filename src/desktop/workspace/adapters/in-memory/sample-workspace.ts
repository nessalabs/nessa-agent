/**
 * The in-memory source's sample index: sections, channels, sessions
 * and their conversations, dated relative to `now` so they always read as
 * recent. Only `in-memory-source.ts` reads this; nothing else in the window
 * knows it exists.
 */
import type {
  Channel,
  WorkspaceIndex,
  Section,
  SessionSummary,
} from "../../model/workspace-index"
import type { Activity, Message, Transcript } from "../../model/transcript"
import { labsSamples } from "./sample-labs"
import { starredSamples } from "./sample-starred"

const minute = 60_000
const samples = [...starredSamples, ...labsSamples]

const sections: Section[] = [
  { id: "starred", name: "Starred" },
  { id: "labs", name: "Nessa Labs" },
  { id: "personal", name: "Personal" },
]

const channel = (
  id: string,
  sectionId: string,
  topic: string,
  isPrivate = false,
): Channel => ({ id, name: id, sectionId, topic, private: isPrivate })

const channels: Channel[] = [
  channel("desktop-app", "starred", "The Tauri desktop window: shell, home, composer"),
  channel("release", "starred", "Cutting, signing and shipping builds", true),
  channel("gateway", "labs", "nessa-gateway and the ACP harness"),
  channel("sdk", "labs", "nessa-sdk: models, lifecycle, public API"),
  channel("design-system", "labs", "nessa_ui components and tokens"),
  channel("onboarding", "labs", "First run and agent detection"),
  channel("reading-list", "personal", "Papers and posts worth the time"),
  channel("home-lab", "personal", "The NAS, the Pi, and backups"),
  channel("writing", "personal", "Drafts and notes"),
  channel("dotfiles", "personal", "Shell, editor and machine setup"),
]

/** The sample workspace as it stands at `now`: its index, and every conversation. */
export function sampleWorkspace(now: number): {
  index: WorkspaceIndex
  transcripts: Map<string, Transcript>
} {
  const ago = (minutes: number) => now - minutes * minute
  const sessions: SessionSummary[] = samples.map((sample) => ({
    id: sample.id,
    channelId: sample.channelId,
    title: sample.title,
    model: sample.model,
    status: sample.status ?? "idle",
    startedAt: ago(sample.started ?? sample.messages[0]?.[1] ?? sample.updated),
    updatedAt: ago(sample.updated),
    preview: sample.preview,
    ...(sample.now === undefined ? {} : { now: sample.now }),
    pinned: sample.pinned ?? false,
    unread: sample.unread ?? false,
    revision: 1,
  }))
  const transcripts = new Map(
    samples.map((sample): [string, Transcript] => {
      const messages: Message[] = sample.messages.map(
        ([role, minutes, parts], index) => ({
          id: `${sample.id}-${index + 1}`,
          role,
          at: ago(minutes),
          parts,
        }),
      )
      const activity: Activity | null = sample.activity
        ? { label: sample.activity[0], since: now - sample.activity[1] * 1000 }
        : null
      return [
        sample.id,
        {
          sessionId: sample.id,
          messages,
          activity,
          approval: sample.approval ?? null,
          revision: 1,
        },
      ]
    }),
  )
  return { index: { sections, channels, sessions }, transcripts }
}
