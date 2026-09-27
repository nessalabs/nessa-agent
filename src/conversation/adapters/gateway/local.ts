import type { ConversationGateway } from "../../application/ports"
import {
  attachFiles,
  chooseApproval,
  chooseModel,
  changeUpload,
  forgetStoredUploads,
  removeFile,
  closeConversation,
  openConversation,
  chooseModel,
  openListed,
  setActive,
  moveActive,
  setDraft,
} from "../../application/usecases"

/** Local drafts and tabs; remote operations use ConversationEffects. */
export const localConversationGateway: ConversationGateway = {
  openConversation,
  chooseModel,
  openListed,
  attachFiles,
  changeUpload,
  forgetStoredUploads,
  removeFile,
  closeConversation,
  chooseModel,
  chooseApproval,
  setDraft,
  setActive,
  moveActive,
}
