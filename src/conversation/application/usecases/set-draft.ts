import { validDraftAttachments, type MessageContent } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { findConversation, replaceConversation, withDraft } from "../internal"

/**
 * Replace a draft's prose. Its files are not the caller's to say.
 *
 * The composer rebuilds the draft from what it last rendered, and the store can
 * move between that render and the next keystroke: an upload settles, a tile is
 * removed, the draft is sent. Taking the caller's file parts would undo whichever
 * of those happened — a stored file back to uploading, a removed or sent file
 * back in the draft. So the prose comes from the caller and the files come from
 * the store, exactly as they are. A file enters a draft through `attachFiles`
 * and leaves through `removeFile` or a send, and nowhere else.
 */
export function setDraft(
  tabs: LocalTabs,
  input: { draft: MessageContent; id?: string },
): LocalTabs {
  const current = findConversation(tabs, input.id ?? tabs.activeId)
  if (!current) return tabs
  const draft = [
    ...input.draft.filter((part) => part.type === "text" || part.type === "pasted-text"),
    ...current.draft.filter((part) => part.type === "file"),
  ]
  if (!validDraftAttachments(draft)) return tabs
  return replaceConversation(tabs, withDraft(current, draft))
}
