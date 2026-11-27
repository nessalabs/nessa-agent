/**
 * The decisions agents in the sample workspace wait on, by chat: what the
 * Decisions view and the chats' pointers to it show in a preview. They run
 * from a quick yes or no to a technical choice with every fact an agent can
 * give, so the view is seen at each depth. A chat listed here waits on the
 * person (`needs-you`), as a chat whose agent asked does.
 */
import type { OwnerAsk } from "../../model/workspace-index"

export const sampleAsks: Readonly<Record<string, OwnerAsk>> = {
  "rate-limit": {
    askId: "sample-rate-limit",
    question: "Where should the rate limit be counted?",
    summary:
      "Clients get 429s with no hint of when to retry. Adding RateLimit headers means one place has to own the count, and today three could.",
    choices: [
      {
        label: "In the gateway",
        consequence: "One counter per client, before any agent work starts.",
        effort: "small",
        undo: "easy",
        pros: ["Headers are exact", "One file changes"],
        cons: ["Resets when the gateway restarts"],
      },
      {
        label: "In the event store",
        consequence: "The count survives restarts and is shared by every gateway.",
        effort: "medium",
        undo: "costly",
        pros: ["Survives restarts", "Works with several gateways"],
        cons: ["A write on every request", "Needs a schema upgrade"],
      },
      {
        label: "At the proxy",
        consequence:
          "The proxy counts and adds the headers; the gateway never sees them.",
        effort: "large",
        undo: "costly",
        pros: ["No gateway code"],
        cons: ["Only works behind our proxy", "Local installs get no limit"],
      },
    ],
    recommended: 0,
    reason:
      "There is one gateway per install today, and a restart forgetting the count only ever lets a client in early. Moving it later is a single function.",
    autoPick: "never",
  },
  "tauri-bump": {
    askId: "sample-tauri-bump",
    question: "Bump Tauri to 2.4 before or after 0.9 ships?",
    summary:
      "2.4 fixes the WebKit focus bug in the composer, but it also changes how the updater signs builds.",
    choices: [
      {
        label: "After 0.9",
        consequence: "0.9 ships on the build we have tested; the bump lands next week.",
        undo: "easy",
      },
      {
        label: "Before 0.9",
        consequence:
          "0.9 gets the focus fix, and the release waits about two days for signing tests.",
        undo: "costly",
      },
    ],
    recommended: 0,
    reason:
      "The focus bug has a workaround, and a signing change is the riskiest thing to add the week of a release.",
  },
  "intro-sound": {
    askId: "sample-intro-sound",
    question: "Play the intro sound on first launch?",
    choices: [
      { label: "Play it", consequence: "Quiet, once, and never again.", undo: "easy" },
      {
        label: "Leave it off",
        consequence: "People can turn it on in Settings.",
        undo: "easy",
      },
    ],
    recommended: 1,
    reason: "Most first launches happen in an office.",
  },
  "nas-backup": {
    askId: "sample-nas-backup",
    question: "Delete the 2023 snapshots to make room on B2?",
    summary:
      "The bucket is at 96%. Tonight's backup needs about 180 GB more than is free.",
    choices: [
      {
        label: "Delete 2023",
        consequence: "Frees 410 GB. Those snapshots are gone for good.",
        undo: "no",
      },
      {
        label: "Raise the cap",
        consequence: "Adds about $4 a month; nothing is deleted.",
        undo: "easy",
      },
    ],
    recommended: 0,
    reason:
      "Every file in 2023 is also in the 2024 snapshots, and the cap went up twice this year already.",
  },
}
