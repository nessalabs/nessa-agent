/**
 * The scenario the conversation stress check runs (`conversation-stress.mjs`),
 * built from its numbers rather than kept as a file: how many reviews one
 * turn asks in a row, and how large the large one is. The check writes it to
 * a file and starts the scripted agent on that file.
 *
 * Every prompt the check sends names its turn by one of `STRESS`'s phrases.
 * A turn's text echoes the prompt it answers (`${prompt.text}`), so the
 * window and the gateway's view can be read for exactly that round.
 */

/** The phrase each kind of turn is asked by. */
export const STRESS = {
  report: "[Thread report]",
  reply: "stress reply",
  permission: "stress permission",
  reviews: "stress reviews",
  large: "stress large review",
  wait: "stress wait",
  pause: "stress pause",
  cut: "stress cut",
  fail: "stress fail",
  delegate: "stress delegate",
  long: "stress long",
}

/**
 * How the gateway opens a turn it carries on after a restart. A turn the
 * check cuts off never ends on its own, so a restart can never come too late;
 * its successor is told apart by this notice and finishes.
 */
const CUT_OFF = "[Nessa restarted while you were working"

/** What a turn says, by kind. A check looks for these, so they are owned here. */
export const SAYS = {
  echo: "Echo: ",
  allowed: "Allowed.",
  denied: "Denied.",
  reviewed: "All reviews answered.",
  waiting: "Working until stopped.",
  pausing: "Pausing.",
  resumed: "Finished after the pause.",
  beforeFailure: "Before the failure.",
  failure: "the stress scenario failed this turn",
  delegated: "Delegated to ",
  report: "Report received.",
  long: "Long answer: ",
}

/** The tool every review asks for: the test server's own, so an allow can run it. */
const TOOL = "report_rows"

const review = (title, args, allowedText) => ({
  do: "permission",
  tool: TOOL,
  arguments: args,
  title,
  on: {
    "allow-once": [
      { do: "tool", tool: TOOL, arguments: {} },
      { do: "text", chunks: [allowedText] },
    ],
    "deny-once": [{ do: "text", chunks: [SAYS.denied] }],
  },
})

/**
 * The scenario for `reviews` reviews in one turn, one review of about
 * `largeBytes` bytes, turns that pause for `pauseMs`, and long answers
 * streamed as `longChunks` chunks of about 2 KB, so a conversation's history
 * grows fast.
 */
export function stressScenario({
  reviews = 20,
  largeBytes = 24_000,
  pauseMs = 3_000,
  longChunks = 40,
} = {}) {
  const many = Array.from({ length: Math.max(1, reviews) }, (_, index) =>
    review(`Review ${index + 1} of ${reviews}`, { index }, `Allowed ${index + 1}.`),
  )
  return {
    turns: [
      // First: a report quotes the child's reply, which can hold any phrase.
      {
        when: STRESS.report,
        steps: [{ do: "text", chunks: [SAYS.report] }, { do: "end" }],
      },
      // Before the cut turn: the successor quotes the cut prompt after it.
      {
        when: CUT_OFF,
        steps: [{ do: "text", chunks: [SAYS.resumed] }, { do: "end" }],
      },
      {
        when: STRESS.reply,
        steps: [{ do: "text", chunks: [SAYS.echo, "${prompt.text}"] }, { do: "end" }],
      },
      {
        when: STRESS.long,
        steps: [
          {
            do: "text",
            chunks: [
              SAYS.long,
              "${prompt.text}",
              ...Array.from(
                { length: longChunks },
                (_, index) => ` ${index} ${"x".repeat(2_000)}`,
              ),
            ],
          },
          { do: "end" },
        ],
      },
      {
        when: STRESS.permission,
        steps: [
          { do: "text", chunks: ["Asking: ${prompt.text}"] },
          review("Report the rows", {}, SAYS.allowed),
          { do: "end" },
        ],
      },
      {
        when: STRESS.reviews,
        steps: [...many, { do: "text", chunks: [SAYS.reviewed] }, { do: "end" }],
      },
      {
        when: STRESS.large,
        steps: [
          review("A large review", { body: "x".repeat(largeBytes) }, SAYS.allowed),
          { do: "end" },
        ],
      },
      {
        when: STRESS.wait,
        steps: [{ do: "text", chunks: [SAYS.waiting] }, { do: "wait-cancel" }],
      },
      {
        when: STRESS.pause,
        steps: [
          { do: "text", chunks: [SAYS.pausing] },
          { do: "pause", ms: pauseMs },
          { do: "text", chunks: [SAYS.resumed] },
          { do: "end" },
        ],
      },
      {
        when: STRESS.cut,
        steps: [{ do: "text", chunks: [SAYS.pausing] }, { do: "wait-cancel" }],
      },
      {
        when: STRESS.fail,
        steps: [
          { do: "text", chunks: [SAYS.beforeFailure] },
          { do: "fail", message: SAYS.failure },
        ],
      },
      {
        when: STRESS.delegate,
        steps: [
          {
            do: "tool",
            server: "nessa-agents",
            tool: "create_thread",
            arguments: {},
            save: "thread",
          },
          {
            do: "tool",
            server: "nessa-agents",
            tool: "send_message",
            arguments: {
              threadId: "${thread.threadId}",
              message: `${STRESS.reply} from the parent`,
            },
            save: "sent",
          },
          { do: "text", chunks: [SAYS.delegated, "${thread.threadId}"] },
          { do: "end" },
        ],
      },
    ],
  }
}
