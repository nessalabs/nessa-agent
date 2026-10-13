import { AttachmentResource } from "../../model"
import { expect, it } from "vitest"
import { emptyLocalTabs } from "../local-tabs"
import { attachFiles, changeUpload } from "../usecases/attachments"
import { removeFile } from "../usecases/attachments"
import { openConversation } from "../usecases/open-conversation"
import { beginSend, failSend } from "../usecases/send-draft"
import { boundSentPreviews } from "../usecases/release-uploads"
import { restoreConversationTabs } from "../saved-tabs"
import { attachmentResourceUsage } from "./attachment-resources"

const input: AttachmentResource = AttachmentResource.create({
  name: "image.png",
  mimeType: "image/png",
  size: 40,
  previewUrl: "blob:resource",
  path: null,
})
const image = {
  digest: `sha256:${"ab".repeat(32)}`,
  mimeType: "image/png" as const,
  size: 1,
}
function placed(tabs: ReturnType<typeof emptyLocalTabs>, id = "c0") {
  const part = tabs.conversations.find((tab) => tab.id === id)!.draft[0]
  if (!part || part.type !== "file") throw new Error("Attachment was not admitted")
  return part
}
function accepted(
  tabs: ReturnType<typeof emptyLocalTabs>,
  executionId: string,
): ReturnType<typeof emptyLocalTabs> {
  const placement = placed(tabs)
  tabs = changeUpload(tabs, {
    conversationId: "c0",
    placementId: placement.placementId,
    to: "uploading",
  })
  tabs = changeUpload(tabs, {
    conversationId: "c0",
    placementId: placement.placementId,
    to: "stored",
    image,
  })
  tabs = beginSend(tabs, {
    conversationId: "c0",
    executionId,
    actionId: executionId,
    mode: "queued",
    content: tabs.conversations[0]!.draft,
  })
  return {
    ...tabs,
    conversations: tabs.conversations.map((tab) => ({
      ...tab,
      turns: tab.turns.map((turn) =>
        turn.from === "user" ? { ...turn, receipt: "accepted" as const } : turn,
      ),
    })),
  }
}

it("AL1/AL2: re-admission mints a placement and discards the caller's old upload", () => {
  let tabs = attachFiles(emptyLocalTabs(), [input], "c0")
  const original = placed(tabs)
  tabs = changeUpload(tabs, {
    conversationId: "c0",
    placementId: original.placementId,
    to: "uploading",
  })
  const retained = placed(tabs)
  tabs = removeFile(tabs, original.placementId)
  tabs = attachFiles(
    tabs,
    [retained].map((file) => file.resource),
    "c0",
  )
  const replacement = placed(tabs)
  expect(replacement.resource).toBe(original.resource)
  expect(replacement.placementId).not.toBe(original.placementId)
  expect(replacement.upload).toEqual({ status: "not-started" })
  for (const change of [
    { to: "stored" as const, image },
    { to: "failed" as const, reason: "interrupted" as const },
  ])
    expect(
      changeUpload(tabs, {
        conversationId: "c0",
        placementId: original.placementId,
        ...change,
      }),
    ).toBe(tabs)
})

it("AL4: sharing resources creates independent placements and preserves within-draft dedup", () => {
  let tabs = attachFiles(emptyLocalTabs(), [input], "c0")
  const first = placed(tabs)
  expect(attachFiles(tabs, [input], "c0")).toBe(tabs)
  tabs = openConversation(tabs)
  tabs = attachFiles(tabs, [input], "c1")
  const second = placed(tabs, "c1")
  expect(second.resource).toBe(first.resource)
  expect(second.placementId).not.toBe(first.placementId)
  const changed = changeUpload(tabs, {
    conversationId: "c1",
    placementId: second.placementId,
    to: "uploading",
  })
  expect(changed.conversations[0]).toBe(tabs.conversations[0])
})

it("AL6: a refused submission returns the original placement to its draft", () => {
  let tabs = attachFiles(emptyLocalTabs(), [input], "c0")
  const original = placed(tabs)
  tabs = changeUpload(tabs, {
    conversationId: "c0",
    placementId: original.placementId,
    to: "uploading",
  })
  tabs = changeUpload(tabs, {
    conversationId: "c0",
    placementId: original.placementId,
    to: "stored",
    image,
  })
  tabs = beginSend(tabs, {
    conversationId: "c0",
    executionId: "execution",
    actionId: "action",
    mode: "queued",
    content: tabs.conversations[0]!.draft,
  })
  tabs = failSend(tabs, "c0", "execution", "refused", { kind: "refused", reupload: true })
  expect(placed(tabs)).toMatchObject({
    placementId: original.placementId,
    resource: original.resource,
    upload: { status: "not-started" },
  })
})

it("AL7: a shared draft keeps accepted bytes needed and out of preview eviction", () => {
  let tabs = accepted(attachFiles(emptyLocalTabs(), [input], "c0"), "first")
  tabs = attachFiles(tabs, [input], "c0")
  const usage = attachmentResourceUsage(tabs)
  expect([...usage.needed]).toEqual([input])
  expect(usage.previews.size).toBe(0)
  expect(boundSentPreviews(tabs, 0)).toBe(tabs)
})

it("AL8: preview accounting counts one resource and eviction removes every accepted reference", () => {
  let tabs = accepted(attachFiles(emptyLocalTabs(), [input], "c0"), "first")
  tabs = accepted(attachFiles(tabs, [input], "c0"), "second")
  expect([...attachmentResourceUsage(tabs).previews]).toEqual([input])
  expect(boundSentPreviews(tabs, input.size)).toBe(tabs)
  const evicted = boundSentPreviews(tabs, 0)
  expect(
    evicted.conversations[0]!.turns.flatMap((turn) =>
      turn.from === "user" ? turn.content.map((part) => part.type) : [],
    ),
  ).toEqual(["image-reference", "image-reference"])
  expect(attachmentResourceUsage(evicted).previews.size).toBe(0)
})

it("AL9/AL10: placement numbering is store-local, survives restoration and rejected batches", () => {
  const first = attachFiles(emptyLocalTabs(), [input], "c0")
  const independent = attachFiles(emptyLocalTabs(), [input], "c0")
  expect(placed(first).placementId).toBe(placed(independent).placementId)
  expect(attachFiles(first, [input, input], "c0")).toBe(first)
  const restored = restoreConversationTabs(first, { tabs: [] })
  const next = attachFiles(restored, [input], restored.activeId)
  expect(placed(next).placementId).not.toBe(placed(first).placementId)
  expect(next.nextPlacementId).toBe(first.nextPlacementId + 1)
})

function admissionTypeChecks() {
  // @ts-expect-error Placement identity is minted by admission, not supplied with a resource.
  const placement: AttachmentResource = { ...input, placementId: "reused" }
  // @ts-expect-error Upload state belongs to an admitted placement.
  const upload: AttachmentResource = { ...input, upload: { status: "stored", image } }
  return [placement, upload]
}
void admissionTypeChecks

it("AL13: indexed admission ignores caller array methods", () => {
  const files = [input]
  Object.defineProperty(files, "map", { value: () => [] })
  Object.defineProperty(files, Symbol.iterator, {
    value: () => {
      throw new Error("caller iterator")
    },
  })
  const tabs = attachFiles(emptyLocalTabs(), files, "c0")
  expect(placed(tabs)).toEqual({
    type: "file",
    resource: input,
    placementId: "a1",
    upload: { status: "not-started" },
  })
  expect(placed(tabs).resource).toBe(input)
  expect(tabs.nextPlacementId).toBe(2)
})
