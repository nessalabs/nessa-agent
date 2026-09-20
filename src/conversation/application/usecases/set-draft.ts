import { validDraftAttachments, type MessageContent } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { findConversation, replaceConversation, withDraft } from "../internal"

/**
 * Replace a draft's prose, keeping each file's upload state as the store has it.
 *
 * The composer rebuilds the draft from what it last rendered, and an upload can
 * settle between that render and the next keystroke. Taking the caller's copy of
 * a file part would put a `stored` file back to `uploading` for good. So a file
 * the draft already holds is kept exactly as it is here, and one it does not
 * hold arrives not started — only `changeUpload` moves that state.
 */
export function setDraft(
  tabs: LocalTabs,
  input: { draft: MessageContent; id?: string },
): LocalTabs {
  const current = findConversation(tabs, input.id ?? tabs.activeId)
  if (!current) return tabs
  const held = new Map(
    current.draft.flatMap((part) =>
      part.type === "file" ? [[part.id, part] as const] : [],
    ),
  )
  const draft = input.draft.map((part) =>
    part.type === "file"
      ? (held.get(part.id) ?? { ...part, upload: { status: "not-started" as const } })
      : part,
  )
  if (!validDraftAttachments(draft)) return tabs
  return replaceConversation(tabs, withDraft(current, draft))
}
