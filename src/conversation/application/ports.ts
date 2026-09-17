import type { ConversationView, Submission, SubmissionReceipt } from "./view"
import { type FileAttachment, type MessageContent } from "../model"
import type { LocalTabs } from "./local-tabs"

/** Pure local draft and tab operations. Remote work uses injected effects below. */
export interface ConversationGateway {
  attachFiles(tabs: LocalTabs, files: FileAttachment[], conversationId: string): LocalTabs
  removeFile(tabs: LocalTabs, id: string): LocalTabs
  openConversation(tabs: LocalTabs): LocalTabs
  closeConversation(tabs: LocalTabs, conversationId: string): LocalTabs
  setDraft(tabs: LocalTabs, input: { draft: MessageContent; id?: string }): LocalTabs
  moveActive(tabs: LocalTabs, direction: -1 | 1): LocalTabs
  setActive(tabs: LocalTabs, conversationId: string): LocalTabs
}

/** External effects consumed by conversation commands. */
export interface ConversationEffects {
  create(conversationId: string): Promise<{ conversationId: string }>
  read(conversationId: string): Promise<ConversationView>
  send(input: Submission): Promise<SubmissionReceipt>
  steer(input: Submission): Promise<SubmissionReceipt>
  reorder(
    conversationId: string,
    executionIds: readonly string[],
  ): Promise<"applied" | "unchanged" | "queue_changed" | "priority_conflict">
  remove(conversationId: string, executionId: string): Promise<void>
  answer(
    conversationId: string,
    executionId: string,
    permissionId: string,
    optionId: string,
  ): Promise<void>
  cancel(conversationId: string, executionId: string, permissionId: string): Promise<void>
  close(conversationId: string): Promise<void>
}

/** No live transport accepted this operation; unlike a lost acknowledgement, no message was submitted. */
export class ConversationUnavailableError extends Error {
  constructor() {
    super("Not connected to the gateway. Your message was not sent.")
    this.name = "ConversationUnavailableError"
  }
}
