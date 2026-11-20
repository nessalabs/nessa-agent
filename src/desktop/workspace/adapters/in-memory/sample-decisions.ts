/**
 * The sample workspace's decisions: a few settled ones for the split-panes
 * chat, newest first as the gateway lists them, so Chat context and the
 * decisions dialog have rows to show. Every
 * other sample chat has none. Times are minutes before the window opened.
 */
import type { DecisionSource } from "../../application/ports"
import type { Decision } from "../../model/decisions"

const minute = 60_000

export function sampleDecisions({ now }: { now: () => number }): DecisionSource {
  const opened = now()
  const at = (minutesAgo: number) => opened - minutesAgo * minute
  const bySession: Readonly<Record<string, readonly Decision[]>> = {
    "split-panes": [
      {
        ask: {
          askId: "split-panes-cap",
          question: "How many panes should the workspace hold?",
          choices: [
            {
              label: "Four",
              consequence: "Fits a 14-inch screen; a fifth opens in place of the oldest.",
            },
            {
              label: "Six",
              consequence: "Room for more, but each pane gets narrow on a laptop.",
            },
          ],
          recommended: 0,
          reason: "Four keeps every pane wide enough to read code.",
        },
        askedAtMs: at(34),
        outcome: { state: "decided", text: "Four", choice: 0, atMs: at(31) },
      },
      {
        ask: {
          askId: "split-panes-drop",
          question: "Where should a dragged chat land?",
          choices: [
            {
              label: "Beside the target",
              consequence: "Opens next to the pane it is dropped on.",
            },
            { label: "At the end", consequence: "Always opens as the last pane." },
          ],
          recommended: 0,
          reason: "It lands where the person points.",
        },
        askedAtMs: at(52),
        outcome: {
          state: "decided",
          text: "Beside the target. Keep the drop line visible while dragging.",
          choice: 0,
          atMs: at(47),
        },
      },
      {
        ask: {
          askId: "split-panes-sizes",
          question: "Should pane sizes be kept per channel?",
          choices: [
            {
              label: "Per channel",
              consequence: "Each channel opens with its own layout.",
            },
            {
              label: "One for all",
              consequence: "Every channel shares the last layout.",
            },
          ],
          recommended: 0,
          reason: "Channels are used for different work.",
        },
        askedAtMs: at(58),
        outcome: { state: "replaced", atMs: at(52) },
      },
      {
        ask: {
          askId: "split-panes-fractions",
          question: "Lay panes out as fractions or fixed widths?",
          choices: [
            {
              label: "Fractions",
              consequence: "Panes keep their share when the window resizes.",
            },
            {
              label: "Fixed widths",
              consequence: "Panes keep their size; the last one absorbs the change.",
            },
          ],
          recommended: 0,
          reason: "A third pane never squeezes the first.",
        },
        askedAtMs: at(80),
        outcome: {
          state: "decided",
          text: "Whatever keeps the sidebar steady.",
          atMs: at(76),
        },
      },
    ],
  }
  return {
    decisions: async (sessionId) => bySession[sessionId] ?? [],
  }
}
