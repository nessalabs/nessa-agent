import type { ConversationGateway } from "../../application/ports"
import {
  attachFiles,
  changeUpload,
  forgetStoredUploads,
  removeFile,
  closeConversation,
  openConversation,
  openListed,
  setActive,
  moveActive,
  setDraft,
} from "../../application/usecases"

/** Local drafts and tabs; remote operations use ConversationEffects. */
export const localConversationGateway: ConversationGateway = {
  openConversation,
  openListed,
  attachFiles,
  changeUpload,
  forgetStoredUploads,
  removeFile,
  closeConversation,
  setDraft,
  setActive,
  moveActive,
}
