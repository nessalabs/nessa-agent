import { validDraftAttachments, type FileAttachment } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { findConversation, replaceConversation, withDraft } from "../internal"

/** Append local preview files to the requested conversation, atomically within limits. */
export function attachFiles(
  tabs: LocalTabs,
  files: FileAttachment[],
  conversationId: string,
): LocalTabs {
  const current = findConversation(tabs, conversationId)
  if (!current || files.length === 0) return tabs
  const draft = [...current.draft, ...files]
  if (!validDraftAttachments(draft)) return tabs
  return replaceConversation(tabs, withDraft(current, draft))
}

/** Remove a file preview from the active conversation without changing prose. */
export function removeFile(tabs: LocalTabs, id: string): LocalTabs {
  const current = findConversation(tabs, tabs.activeId)
  if (!current || !current.draft.some((part) => part.type === "file" && part.id === id))
    return tabs
  return replaceConversation(
    tabs,
    withDraft(
      current,
      current.draft.filter((part) => part.type !== "file" || part.id !== id),
    ),
  )
}
