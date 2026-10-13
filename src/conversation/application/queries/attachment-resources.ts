import type { AttachmentResource } from "../../model"
import type { LocalTabs } from "../local-tabs"

/** Complete ownership snapshot of retained original resources in one store. */
export type AttachmentResourceUsage = {
  needed: ReadonlySet<AttachmentResource>
  previews: ReadonlySet<AttachmentResource>
}

/** Classify shared references together; AL7/AL8 in attachment-resources.test.ts enforce needed-byte precedence. */
export function attachmentResourceUsage(tabs: LocalTabs): AttachmentResourceUsage {
  const needed = new Set<AttachmentResource>()
  const previews = new Set<AttachmentResource>()
  for (const conversation of tabs.conversations) {
    for (const part of conversation.draft)
      if (part.type === "file") needed.add(part.resource)
    for (const turn of conversation.turns) {
      if (turn.from !== "user") continue
      const accepted = ["accepted", "queued", "delivered"].includes(turn.receipt)
      for (const part of turn.content) {
        if (part.type !== "file") continue
        if (!accepted) needed.add(part.resource)
        else if (part.upload.status === "stored") previews.add(part.resource)
      }
    }
  }
  for (const resource of needed) previews.delete(resource)
  return { needed, previews }
}
