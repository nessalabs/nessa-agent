/** Sample sessions in #checkout-agent: a conversation that runs an experiment. */
import { models, ran, read, text, type SampleSession } from "./sample-content"

const experiment = {
  kind: "widget",
  plugin: "experiment",
  id: "checkout-hillclimb",
} as const

export const experimentSamples: readonly SampleSession[] = [
  {
    id: "checkout-hillclimb",
    channelId: "checkout-agent",
    title: "Hill-climb checkout support",
    model: models.opus,
    status: "running",
    updated: 0,
    started: 192,
    preview: "Up 13.3 points to 71.6 — seven changes kept, four evaluating.",
    now: "Six agents hill-climbing checkout support; four changes evaluating",
    pinned: true,
    activity: ["Watching 4 evaluations", 540],
    messages: [
      [
        "user",
        192,
        [
          text(
            "Our checkout support agent resolves about 58% of the eval. Set up a hill-climb: split the eval, measure the noise, and put a swarm on it — prompt, tools, retrieval, model, harness. Cost per task can't go up more than 10%.",
          ),
        ],
      ],
      [
        "agent",
        188,
        [
          read("evals/checkout-support/cases.jsonl"),
          read("evals/checkout-support/rubric.md"),
          ran("nessa eval run checkout-support --split 120/60 --repeat 2"),
          ran("nessa eval run checkout-support --model opus-5.5 --effort max"),
          text(
            "The baseline scores **58.3** on test, and two reruns agree within **±1.2** — anything smaller counts as flat. The strongest model at max effort reaches 84.1, so there is real headroom.",
          ),
          text(
            "Six agents are on it, one area each. A change is kept only if train and test both clear the noise; train up with test flat is overfitting, and gets reverted.",
          ),
        ],
      ],
      ["user", 20, [text("How's it going? Anything worth merging yet?")]],
      [
        "agent",
        18,
        [
          text(
            "Up **13.3 points** to 71.6 with seven kept changes; the system prompt carried the most. Two things to know:",
          ),
          {
            kind: "list",
            items: [
              "**Model & effort** hasn't produced a keep: its gains cost 22–64% more per task, so Pike is diagnosing failures instead of trying bigger models.",
              "Rule-style prompt edits keep overfitting (#8, #15, #31). Wren is trying one worked example instead.",
            ],
          },
          text(
            "Every step on the path to the best version held on test, so it is safe to merge as it stands. It's live below:",
          ),
          experiment,
        ],
      ],
    ],
  },
]
