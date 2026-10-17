import {
  preparedSubmissionFields,
  executionTargetConversation,
  type PreparedSubmission,
} from "@nessa/client"
import {
  AttachmentStagingError,
  ControlFailedError,
  ConversationReadFailedError,
  HeldFileReadError,
  SubmissionRefusedError,
  type ConversationEffects,
  type ConversationFollower,
  type ConversationListFollower,
} from "../../application/ports"
import type { ConversationView } from "../../application/view"
import { STORED_IMAGE_TYPES } from "../../model"

/** The most published files a view lists, newest last, as the gateway's view does. */
const SCENARIO_ARTIFACTS_KEPT = 64

/** Explicit development/test injection only; production composition always uses the gateway. */
export function scenarioEffects(scenario: "echo" | "offline"): ConversationEffects {
  const views = new Map<string, ConversationView>()
  const archived = new Set<string>()
  // Deleted identities are refused for good, as the gateway's tombstones are.
  const deleted = new Set<string>()
  // Who follows each conversation, told again whenever it changes.
  const followers = new Map<string, Set<ConversationFollower>>()
  // The bytes each conversation holds, by the name its view gives them, as
  // the gateway keeps an upload. Nothing is converted, so what was staged is
  // what is held.
  const held = new Map<string, Blob>()
  const heldKey = (
    id: string,
    file: { digest: string; mimeType: string; size: number },
  ) => JSON.stringify([id, file.digest, file.mimeType, file.size])
  /** Lets go of everything `id` holds, as closing or deleting it does on the gateway. */
  const release = (id: string) => {
    for (const key of held.keys())
      if ((JSON.parse(key) as string[])[0] === id) held.delete(key)
  }
  /** Tells `follower` what `id` says now, a task later, as a socket would. */
  const tell = (id: string, follower: ConversationFollower) => {
    void Promise.resolve().then(() => {
      if (!followers.get(id)?.has(follower)) return
      // The port promises a typed word for every view it cannot give, and a
      // substitute that answers with anything else is a substitute the panel
      // could not have been written against. An offline scenario and a
      // conversation this one never opened are both "no view, and nothing more
      // to say about it".
      if (deleted.has(id)) return follower.failed("deleted")
      let view: ConversationView
      try {
        view = structuredClone(get(id))
      } catch (error) {
        // The scenario's own sentence stays the cause, so a developer reading
        // the console learns which backend refused and why.
        return follower.failed(
          "unavailable",
          new ConversationReadFailedError("unavailable", error),
        )
      }
      follower.view(view)
    })
  }
  const changed = (id: string) => {
    for (const follower of followers.get(id) ?? []) tell(id, follower)
  }
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
  const listFollowers = new Set<{
    archived: boolean
    follower: ConversationListFollower
  }>()
  const listing = (listArchived: boolean) => {
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
        last?.parts.filter((part) => part.kind === "text").at(-1)?.text || last?.userText
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
    // A scenario holds far fewer than the gateway's bound: its list is whole.
    return {
      conversations: rows.filter(
        (row) => row.archived === listArchived && row.preview !== null,
      ),
      complete: true,
    }
  }
  const tellList = (registration: {
    archived: boolean
    follower: ConversationListFollower
  }) => {
    void Promise.resolve().then(() => {
      if (!listFollowers.has(registration)) return
      try {
        registration.follower.list(listing(registration.archived))
      } catch (error) {
        registration.follower.failed(
          error instanceof ConversationReadFailedError ? error.reason : "unavailable",
          error,
        )
      }
    })
  }
  const listsChanged = () => {
    for (const registration of listFollowers) tellList(registration)
  }
  const submit = async (input: PreparedSubmission) => {
    preparedSubmissionFields(input)
    if (deleted.has(input.conversationId))
      throw new SubmissionRefusedError("conversation-deleted")
    // A conversation somebody is writing in is not archived, as on the gateway.
    archived.delete(input.conversationId)
    const view = get(input.conversationId)
    if (!view.messages.some((message) => message.executionId === input.executionId)) {
      // Demo behaviour only, nothing a real agent is promised to do: the echo
      // agent publishes back every image it was sent, under the turn that
      // sent it, so a published file can be shown and saved with no gateway.
      // Its view keeps the newest SCENARIO_ARTIFACTS_KEPT, as the gateway's
      // does; the bytes stay held, as a record left out of the list does.
      input.attachments.forEach((image, index) =>
        view.artifacts.push({
          executionId: input.executionId,
          name: `echo-${index + 1}.${image.mimeType.slice("image/".length)}`,
          digest: image.digest,
          mimeType: image.mimeType,
          size: image.size,
        }),
      )
      view.artifacts.splice(
        0,
        Math.max(0, view.artifacts.length - SCENARIO_ARTIFACTS_KEPT),
      )
      view.messages.push({
        executionId: input.executionId,
        userText: input.text,
        attachments: input.attachments.map((image) => ({ ...image })),
        files: input.files.map((file) => ({ ...file })),
        parts: [
          { offset: 0, kind: "thought", text: "", toolId: "", noticeId: "" },
          { offset: 1, kind: "text", text: input.text, toolId: "", noticeId: "" },
        ],
        status: "completed",
      })
      view.transcriptState = "complete"
      view.revision = String(Number(view.revision) + 1)
      changed(input.conversationId)
      listsChanged()
    }
    return {
      conversationId: input.conversationId,
      executionId: input.executionId,
      disposition: "queued",
    }
  }
  return {
    async setApprovalMode() {
      throw new Error("Scenario does not offer approval-mode changes.")
    },
    async create(conversationId) {
      if (scenario === "offline") throw new Error("Scenario: backend offline")
      // A deleted identity is never reopened, as the gateway's tombstone refuses it.
      if (deleted.has(conversationId))
        throw new SubmissionRefusedError("conversation-deleted")
      if (!views.has(conversationId))
        views.set(conversationId, {
          conversationId,
          approvalMode: "ask",
          approvalModes: [
            { id: "ask", name: "Provider asks", description: "Scenario mode." },
          ],
          // The gateway derives titles; a scenario has no rule to derive one by.
          title: null,
          artifacts: [],
          notes: [],
          unreadable: [],
          unreadableTruncated: false,
          revision: "0",
          messages: [],
          pending: [],
          reviewsWaiting: [],
          questionsWaiting: [],
          permissions: [],
          questions: [],
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
              nativeHooks: "off",
              compactionReporting: "unsupported_not_implemented",
              modelSwitchReporting: "unsupported_not_implemented",
              permissionDeferral: "unsupported_not_implemented",
              elicitationForwarding: "unknown",
              preToolPolicy: "unsupported_not_implemented",
              policyEndTurn: "unsupported",
              policyCloseSession: "unsupported",
              incomingElicitation: "unsupported",
            },
          },
          lifecycle: { phase: "attached" },
          truncated: false,
          queueComplete: true,
          transcriptState: "complete_empty",
        })
      return { conversationId }
    },
    async list(listArchived) {
      return listing(listArchived)
    },
    followList(archived, follower) {
      const registration = { archived, follower }
      listFollowers.add(registration)
      tellList(registration)
      return () => {
        listFollowers.delete(registration)
      }
    },
    async archive(id, archive) {
      control(id)
      // As on the gateway: a conversation nothing was said in is not archived.
      if (!views.get(id)?.messages.length) return false
      const changed = archived.has(id) !== archive
      if (archive) archived.add(id)
      else archived.delete(id)
      if (changed) listsChanged()
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
      release(id)
      changed(id)
      listsChanged()
    },
    follow(id, follower) {
      const following = followers.get(id) ?? new Set<ConversationFollower>()
      followers.set(id, following)
      following.add(follower)
      tell(id, follower)
      return () => {
        following.delete(follower)
      }
    },
    submit,
    async stageAttachment(id, file, bytes, signal) {
      get(id)
      if (signal.aborted) throw new AttachmentStagingError("interrupted")
      // Converts nothing: a scenario has no image library. It answers as a
      // gateway would for the four encodings that need no conversion, and
      // says so for anything that would; what it keeps is what was sent.
      const mimeType = STORED_IMAGE_TYPES.find((type) => type === file.mimeType)
      if (!mimeType) throw new AttachmentStagingError("unsupported-image")
      const stored = { digest: file.digest, mimeType, size: file.size }
      held.set(heldKey(id, stored), bytes)
      return stored
    },
    async readHeldFile(id, file, signal) {
      if (scenario === "offline")
        throw new HeldFileReadError("unavailable", new Error("Scenario: backend offline"))
      if (deleted.has(id)) throw new HeldFileReadError("deleted")
      const bytes = held.get(heldKey(id, file))
      if (!bytes) throw new HeldFileReadError("not-held")
      if (signal.aborted) throw new HeldFileReadError("unavailable")
      return new Blob([bytes], { type: file.mimeType })
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
    async remove(target) {
      executionTargetConversation(target)
    },
    async answer() {},
    async answerQuestion(target) {
      // The scripted gateway has no agent waiting, so answering only closes the
      // question the way a real one would.
      const view = get(target.conversationId)
      view.questions = view.questions.filter(
        (question) => question.questionId !== target.questionId,
      )
    },
    async cancel() {},
    async stop(target) {
      const id = executionTargetConversation(target)
      const executionId = target.executionId
      const view = get(id)
      view.messages = view.messages.map((message) =>
        message.executionId === executionId &&
        (message.status === "running" || message.status === "queued")
          ? { ...message, status: "cancelled" }
          : message,
      )
      view.pending = view.pending.filter((item) => item.executionId !== executionId)
    },
    async close(id) {
      const view = get(id)
      view.messages = view.messages.map((message) =>
        message.status === "running" || message.status === "queued"
          ? { ...message, status: "cancelled" }
          : message,
      )
      view.pending = []
      view.permissions = []
      release(id)
    },
  }
}
