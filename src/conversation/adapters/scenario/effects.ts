import { imageAttachmentsProblem, linkedFilesProblem } from "@nessa/client"
import {
  AttachmentStagingError,
  ControlFailedError,
  ConversationReadFailedError,
  SubmissionRefusedError,
  type ConversationEffects,
} from "../../application/ports"
import type { ConversationView, Submission } from "../../application/view"
import { STORED_IMAGE_TYPES } from "../../model"

/** Explicit development/test injection only; production composition always uses the gateway. */
export function scenarioEffects(scenario: "echo" | "offline"): ConversationEffects {
  const views = new Map<string, ConversationView>()
  const archived = new Set<string>()
  // Deleted identities are refused for good, as the gateway's tombstones are.
  const deleted = new Set<string>()
  const control = (id: string) => {
    // Offline is a gateway that cannot be reached: the command never leaves.
    if (scenario === "offline") throw new ControlFailedError("not-connected", "refused")
    if (deleted.has(id)) throw new ControlFailedError("conversation-deleted", "refused")
    if (!views.has(id)) throw new ControlFailedError("conversation-not-found", "refused")
  }
  const get = (id: string) => {
    if (scenario === "offline") throw new Error("Scenario: backend offline")
    const view = views.get(id)
    if (!view) throw new Error("Scenario conversation not found")
    return view
  }
  const send = async (input: Submission) => {
    if (deleted.has(input.conversationId))
      throw new SubmissionRefusedError("conversation-deleted")
    // A conversation somebody is writing in is not archived, as on the gateway.
    archived.delete(input.conversationId)
    // A substitute that cannot refuse is a substitute that proves nothing. The
    // gateway adapter is held to the client's verdict on a message's images, so
    // this one asks the client the same question and refuses the same way.
    const problem = imageAttachmentsProblem(input.attachments)
    if (problem) {
      const refused = new SubmissionRefusedError("invalid-request")
      refused.message = `Invalid message attachments: ${problem}`
      throw refused
    }
    const pathProblem = linkedFilesProblem(input.files)
    if (pathProblem) {
      const refused = new SubmissionRefusedError("invalid-request")
      refused.message = `Invalid message files: ${pathProblem}`
      throw refused
    }
    const view = get(input.conversationId)
    if (!view.messages.some((message) => message.executionId === input.executionId)) {
      view.messages.push({
        executionId: input.executionId,
        userText: input.text,
        attachments: input.attachments,
        files: input.files,
        parts: [
          { offset: 0, kind: "thought", text: "", toolId: "", noticeId: "" },
          { offset: 1, kind: "text", text: input.text, toolId: "", noticeId: "" },
        ],
        status: "completed",
      })
      view.revision = String(Number(view.revision) + 1)
    }
    return { executionId: input.executionId, disposition: "queued" }
  }
  return {
    async create(conversationId) {
      if (scenario === "offline") throw new Error("Scenario: backend offline")
      // A deleted identity is never reopened, as the gateway's tombstone refuses it.
      if (deleted.has(conversationId))
        throw new SubmissionRefusedError("conversation-deleted")
      if (!views.has(conversationId))
        views.set(conversationId, {
          conversationId,
          // The gateway derives titles; a scenario has no rule to derive one by.
          title: null,
          revision: "0",
          messages: [],
          pending: [],
          permissions: [],
          tools: [],
          // The echo scenario stands in for an agent that takes images, so the
          // attach-and-send path can be driven with no gateway.
          capabilities: {
            queue: true,
            steer: false,
            resume: false,
            permissions: false,
            imageInput: true,
            agentFeatures: {
              permissionDenial: "unknown",
              nativeHookSuppression: "unknown",
              compactionReporting: "unsupported_not_implemented",
              modelSwitchReporting: "unsupported_not_implemented",
              permissionDeferral: "unsupported_not_implemented",
              elicitationForwarding: "unknown",
              preToolPolicy: "unsupported_not_implemented",
              policyEndTurn: "unsupported_not_implemented",
              policyCloseSession: "unsupported_not_implemented",
              incomingElicitation: "unsupported_not_implemented",
            },
          },
          lifecycle: { phase: "attached" },
          truncated: false,
          queueComplete: true,
        })
      return { conversationId }
    },
    async list(listArchived) {
      if (scenario === "offline")
        throw new ConversationReadFailedError(
          "unavailable",
          new Error("Scenario: backend offline"),
        )
      // Newest first, as the gateway lists them; a scenario keeps no clock, so
      // the order conversations were created in stands in for recency. It keeps
      // no titles either — the gateway derives those — so it has none to give.
      const rows = [...views.values()].reverse().map((view, index) => {
        const last = view.messages.at(-1)
        const said =
          last?.parts.filter((part) => part.kind === "text").at(-1)?.text ||
          last?.userText
        // As the gateway says of a message that carried files and no text.
        const folded = said ? said.replace(/\s+/g, " ").trim() : ""
        return {
          conversationId: view.conversationId,
          title: null,
          preview: folded || (last ? "Attachment" : null),
          updatedAtMs: views.size - index,
          running: false,
          archived: archived.has(view.conversationId),
        }
      })
      // As the gateway does: a conversation nothing was said in is not listed.
      return rows.filter((row) => row.archived === listArchived && row.preview !== null)
    },
    async archive(id, archive) {
      control(id)
      // As on the gateway: a conversation nothing was said in is not archived.
      if (!views.get(id)?.messages.length) return false
      const changed = archived.has(id) !== archive
      if (archive) archived.add(id)
      else archived.delete(id)
      return changed
    },
    async delete(id) {
      // The owner deleting it again is answered as the gateway answers: done.
      if (deleted.has(id)) return
      // As the gateway's delete contract says: any code but its two news codes
      // means only that whether it was deleted is not known.
      if (scenario !== "offline" && !views.has(id))
        throw new ControlFailedError(undefined, "unknown")
      control(id)
      views.delete(id)
      archived.delete(id)
      deleted.add(id)
    },
    async read(id) {
      // The port promises a typed reason for every rejected read, and a
      // substitute that answers with anything else is a substitute the panel
      // could not have been written against. An offline scenario and a
      // conversation this one never opened are both "no view, and nothing more
      // to say about it".
      if (deleted.has(id)) throw new ConversationReadFailedError("deleted")
      try {
        return structuredClone(get(id))
      } catch (error) {
        throw new ConversationReadFailedError("unavailable", error)
      }
    },
    send,
    steer: send,
    async stageAttachment(id, file, _bytes, signal) {
      get(id)
      if (signal.aborted) throw new AttachmentStagingError("interrupted")
      // Holds nothing and converts nothing: a scenario has no storage and no
      // image library. It answers as a gateway would for the four encodings
      // that need no conversion, and says so for anything that would.
      const mimeType = STORED_IMAGE_TYPES.find((type) => type === file.mimeType)
      if (!mimeType) throw new AttachmentStagingError("unsupported-image")
      return { digest: file.digest, mimeType, size: file.size }
    },
    async reorder(id, executionIds) {
      const view = get(id)
      if (
        executionIds.length !== view.pending.length ||
        new Set(executionIds).size !== executionIds.length ||
        executionIds.some((id) => !view.pending.some((item) => item.executionId === id))
      )
        return "queue_changed"
      const ordered = executionIds.flatMap((id) =>
        view.pending.filter((item) => item.executionId === id),
      )
      let ordinary = false
      for (const item of ordered) {
        if (item.mode === "queued") ordinary = true
        else if (ordinary) return "priority_conflict"
      }
      if (view.pending.every((item, index) => item.executionId === executionIds[index]))
        return "unchanged"
      view.pending = ordered
      view.revision = String(Number(view.revision) + 1)
      return "applied"
    },
    async remove() {},
    async answer() {},
    async cancel() {},
    async close(id) {
      const view = get(id)
      view.messages = view.messages.map((message) =>
        message.status === "running" || message.status === "queued"
          ? { ...message, status: "cancelled" }
          : message,
      )
      view.pending = []
      view.permissions = []
    },
  }
}
