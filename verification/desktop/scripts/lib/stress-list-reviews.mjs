import assert from "node:assert/strict"

/** Serialized into the window: inspect the actual compact card's controls. */
export function listReviewState(card) {
  const command = card.querySelector(".workspace-request-command")
  return {
    whole: command?.hasAttribute("data-whole") ?? false,
    command: command?.textContent?.replace(/\s+/g, " ").trim() ?? "",
    grants: card.querySelectorAll('[data-answer="once"], [data-answer="always"]').length,
    refusals: card.querySelectorAll('[data-answer="deny"]').length,
    reviewInChat: card.querySelectorAll(".workspace-request-review").length,
  }
}

/** A complete native request must retain its compact grant, not silently use chat. */
export function requireCompactReview(state) {
  assert.equal(state.whole, true, "the native command is not drawn whole")
  assert.match(state.command, /^\$?\s*node --version$/)
  assert.equal(state.grants, 1, "the compact command must offer Allow once")
  assert.equal(state.refusals, 1, "the compact command must offer Deny once")
  assert.equal(state.reviewInChat, 0, "compact approval unexpectedly requires chat")
}

/** A preview may refuse or open chat; an unseen grant is always a failure. */
export function requirePreviewReview(state) {
  assert.equal(state.whole, false, "the oversized request must remain a preview")
  assert.equal(state.grants, 0, "a preview exposed an allow action")
  assert.equal(state.refusals, 1, "the preview must retain Deny once")
  assert.equal(state.reviewInChat, 1, "the preview must offer Review in Chat")
}
