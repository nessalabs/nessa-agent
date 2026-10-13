import type { LocalTabs } from "../local-tabs"

/** Complete ownership snapshot of retained original resources in one store. */
export type AttachmentResourceUsage = {
  needed: ReadonlySet<string>
  previews: ReadonlyMap<string, number>
}

/** Classify shared references together; AL7/AL8 in attachment-resources.test.ts enforce needed-byte precedence. */
export function attachmentResourceUsage(tabs: LocalTabs): AttachmentResourceUsage {
  const needed = new Set<string>()
  const previews = new Map<string, number>()
  for (const conversation of tabs.conversations) {
    for (const part of conversation.draft)
      if (part.type === "file") needed.add(part.resourceId)
    for (const turn of conversation.turns) {
      if (turn.from !== "user") continue
      const accepted = ["accepted", "queued", "delivered"].includes(turn.receipt)
      for (const part of turn.content) {
        if (part.type !== "file") continue
        if (!accepted) needed.add(part.resourceId)
        else if (part.upload.status === "stored") previews.set(part.resourceId, part.size)
      }
    }
  }
  for (const resourceId of needed) previews.delete(resourceId)
  return { needed, previews }
}
