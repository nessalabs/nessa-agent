import type { ReplySource } from "../ports"

/**
 * Stands in for the agent runtime that has not been wired up yet.
 * Only the local gateway (and its tests) may call it.
 */
export function draftReply(prompt: string) {
  return `You said "${prompt}". There is no agent behind this window yet — this is Nessa's chat UI and pill composer running in a floating Tauri panel.`
}

export const standInReply: ReplySource = { reply: draftReply }
