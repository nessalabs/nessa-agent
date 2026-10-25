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
  model: "stress model witness",
  permission: "stress permission",
  reviews: "stress reviews",
  large: "stress large review",
  wait: "stress wait",
  pause: "stress pause",
  cut: "stress cut",
  fail: "stress fail",
  limit: "stress usage limit",
  delegate: "stress delegate",
  await: "stress await",
  watch: "stress watch",
  long: "stress long",
  window: "stress window",
  table: "stress table",
  trace: "stress trace",
  github: "stress github",
  githubShell: "stress shell github",
  channel: "stress channel",
}

/**
 * How the first turn after a switch or a fork begins: the earlier turns the
 * new session has not seen, as one block before the message (the SDK's
 * `HISTORY_PREAMBLE`, `prompt_content.rs`). It quotes those turns, which can
 * hold any phrase, so it is matched first.
 */
export const CARRIED =
  "Earlier turns of this conversation, which this session has not seen"

/**
 * How the gateway opens a turn it carries on after a restart. A turn the
 * check cuts off never ends on its own, so a restart can never come too late;
 * its successor is told apart by this notice and finishes.
 */
const CUT_OFF = "[Nessa restarted while you were working"

/** What a turn says, by kind. A check looks for these, so they are owned here. */
export const SAYS = {
  echo: "Echo: ",
  model: "Native prompt model: ",
  allowed: "Allowed.",
  denied: "Denied.",
  reviewed: "All reviews answered.",
  waiting: "Working until stopped.",
  pausing: "Pausing.",
  resumed: "Finished after the pause.",
  beforeFailure: "Before the failure.",
  failure: "the stress scenario failed this turn",
  delegated: "Delegated to ",
  awaited: "Awaited ",
  report: "Report received.",
  watched: "Watched: ",
  long: "Long answer: ",
  window: "Window: ",
  table: "Tables: ",
  trace: "Traced: ",
  github: "GitHub: ",
  githubShell: "GitHub in the shell: ",
  carried: "Carried on by ",
  channel: "Working in: ",
  channelPrompt: "System prompt: ",
}

/**
 * The two Markdown tables a `table` turn streams, a chunk per line, so each
 * table grows a row at a time while the window draws it. The first fits a
 * reply and carries the short runs a squeezed column used to split letter by
 * letter ("$16.99", "Price"); the second has more columns than a reply is
 * wide, so it scrolls in its own panel.
 */
export const TABLE_LINES = [
  "| Option | Price | Includes | Availability |",
  "| --- | --- | --- | --- |",
  "| Single ticket | $16.99 CAD | One screening, general admission (unassigned seating) | On sale since Sept 29; per-screening status unconfirmed |",
  "| Cine-Maniac Pass | $150 CAD | All Scotiabank screenings, rush priority, social events | |",
  "",
  "| Film | Venue | Date | Time | Runtime | Format | Language | Seats left |",
  "| --- | --- | --- | --- | ---: | --- | --- | ---: |",
  "| The Last Projectionist | Cineplex Scotiabank Theatre | Sat Oct 17 | 12:30 pm | 104 min | DCP | English | 42 |",
  "| Night Shift at the Lighthouse | CineCycle | Sat Oct 17 | 11:59 pm | 88 min | 16 mm | French, English subtitles | 9 |",
]

/**
 * How many reads of one thread a watch turn makes at once: more than one
 * conversation's agent may have waiting, so some are refused and the rest
 * wait out their `waitMs`.
 */
export const WATCH_CALLS = 12

/** How long a `trace` turn waits after each call but the last. */
export const TRACE_PAUSE_MS = 400
/**
 * The calls a `trace` turn makes, in order: what each is called with, which
 * the gateway keeps as its input, and how each ends.
 */
export const TRACE_CALLS = [
  { tool: "report_rows", arguments: {}, status: "completed" },
  { tool: "link_resources", arguments: {}, status: "completed" },
  // The server refuses what it does not take, so this call fails either
  // way; the arguments are there to be kept as its input.
  { tool: "always_fails", arguments: { reason: "stress trace" }, status: "failed" },
]

/**
 * A command printing what GitHub sign-in a shell command has, as
 * `token=<fingerprint> git=<fingerprint>` (`none` for either it lacks): the
 * fingerprint of `GH_TOKEN`, and of the password git's credential helpers
 * give for https://github.com, with no system git config. The same as the
 * scripted agent's `credentials` (`scripted-agent.mjs`).
 */
export const GITHUB_FINGERPRINTS = `node -e '${[
  'const c=require("crypto"),{execFileSync:x}=require("child_process")',
  'const p=(s)=>s?c.createHash("sha256").update(s).digest("hex").slice(0,16):"none"',
  "let g",
  'try{g=/^password=(.*)$/m.exec(x("git",["credential","fill"],{input:"protocol=https\\nhost=github.com\\n\\n",env:{...process.env,GIT_TERMINAL_PROMPT:"0",GIT_CONFIG_NOSYSTEM:"1",GIT_CONFIG_GLOBAL:"/dev/null"},stdio:["pipe","pipe","ignore"],timeout:10000}).toString())?.[1]}catch{}',
  'process.stdout.write("token="+p(process.env.GH_TOKEN)+" git="+p(g))',
].join(";")}'`

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
 * streamed as `longChunks` chunks of `longChunkBytes` bytes each, so a
 * conversation's history grows fast.
 */
export function stressScenario({
  reviews = 20,
  largeBytes = 24_000,
  pauseMs = 3_000,
  longChunks = 40,
  longChunkBytes = 300,
} = {}) {
  const many = Array.from({ length: Math.max(1, reviews) }, (_, index) =>
    review(`Review ${index + 1} of ${reviews}`, { index }, `Allowed ${index + 1}.`),
  )
  return {
    turns: [
      // Before all: the turns carried to a new session quote every phrase.
      // It says the prompt back, so a check sees the earlier turns reached it.
      {
        when: CARRIED,
        steps: [
          {
            do: "text",
            chunks: [SAYS.carried, "${configuration.model}", ": ", "${prompt.text}"],
          },
          { do: "end" },
        ],
      },
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
        when: STRESS.model,
        steps: [
          { do: "text", chunks: [SAYS.model, "${configuration.model}"] },
          { do: "end" },
        ],
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
                (_, index) => ` ${index} ${"x".repeat(longChunkBytes)}`,
              ),
            ],
          },
          { do: "end" },
        ],
      },
      {
        when: STRESS.table,
        steps: [
          {
            do: "text",
            chunks: [
              SAYS.table,
              "${prompt.text}",
              "\n\n",
              ...TABLE_LINES.map((line) => `${line}\n`),
            ],
          },
          { do: "end" },
        ],
      },
      {
        // Calls before the answer, one of them failing, with a pause after
        // each so the bars of the waterfall start apart.
        when: STRESS.trace,
        steps: [
          ...TRACE_CALLS.flatMap(({ tool, arguments: args }, index) => [
            { do: "tool", tool, arguments: args },
            ...(index < TRACE_CALLS.length - 1
              ? [{ do: "pause", ms: TRACE_PAUSE_MS }]
              : []),
          ]),
          { do: "text", chunks: [SAYS.trace, "${prompt.text}"] },
          { do: "end" },
        ],
      },
      {
        // What GitHub sign-in the agent was started with, as fingerprints.
        when: STRESS.github,
        steps: [
          {
            do: "text",
            chunks: [
              SAYS.github,
              "token=${credentials.githubToken} git=${credentials.gitPassword}",
            ],
          },
          { do: "end" },
        ],
      },
      {
        // The same, from a command run by Nessa's own shell tool, where an
        // agent's git and gh actually run.
        when: STRESS.githubShell,
        steps: [
          {
            do: "tool",
            server: "nessa",
            tool: "shell",
            arguments: { command: GITHUB_FINGERPRINTS },
            save: "shell",
          },
          { do: "text", chunks: [SAYS.githubShell, "${shell.stdout}"] },
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
        when: STRESS.watch,
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
            tool: "subscribe",
            arguments: { threadId: "${thread.threadId}", waitMs: 1500 },
            save: "watched",
            times: WATCH_CALLS,
          },
          // Once those end, a read is taken again: refusals hold nothing back.
          {
            do: "tool",
            server: "nessa-agents",
            tool: "subscribe",
            arguments: { threadId: "${thread.threadId}", replay: true, limit: 10 },
            save: "again",
          },
          { do: "text", chunks: [SAYS.watched + "${watched}"] },
          { do: "end" },
        ],
      },
      {
        when: STRESS.window,
        steps: [
          {
            do: "tool",
            server: "nessa-agents",
            tool: "read_window",
            arguments: {},
            save: "window",
          },
          { do: "text", chunks: [SAYS.window + "${window}"] },
          { do: "end" },
        ],
      },
      {
        // Where the session works and what it was told, said back: a turn in
        // a channel is seen to run in the channel's folder, with its notes.
        when: STRESS.channel,
        steps: [
          {
            do: "text",
            chunks: [
              SAYS.channel + "${session.cwd}\n\n",
              SAYS.channelPrompt + "${session.systemPrompt}",
            ],
          },
          { do: "end" },
        ],
      },
      {
        when: STRESS.fail,
        steps: [
          { do: "text", chunks: [SAYS.beforeFailure] },
          { do: "fail", message: SAYS.failure },
        ],
      },
      {
        when: STRESS.limit,
        steps: [
          {
            do: "fail",
            message: "You've hit your weekly limit",
          },
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
      // Sends to a thread and waits for its reply with one subscribe call,
      // as an agent is told to: the reply comes back in that call, and the
      // thread's report is then not sent.
      {
        when: STRESS.await,
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
              message: `${STRESS.reply} awaited by the parent`,
            },
            save: "sent",
          },
          {
            do: "tool",
            server: "nessa-agents",
            tool: "subscribe",
            arguments: {
              threadId: "${thread.threadId}",
              executionId: "${sent.executionId}",
            },
            save: "waited",
          },
          {
            do: "text",
            chunks: [SAYS.awaited, "${thread.threadId}", ": ", "${waited.turn.reply}"],
          },
          { do: "end" },
        ],
      },
    ],
  }
}
