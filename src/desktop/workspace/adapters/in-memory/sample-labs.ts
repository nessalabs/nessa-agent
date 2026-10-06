/** Sample sessions in Nessa Labs and Personal. */
import { fixtureWidget } from "../../../widgets/app/fixture/fixture-widgets"
import { sampleWidgets } from "../../../widgets/fixture/sample-widgets"
import type { WidgetRef } from "../../../widgets/model/widget-ref"
import {
  edited,
  exchange,
  models,
  ran,
  read,
  sampleApprovalOptions,
  searched,
  text,
  type SampleSession,
} from "./sample-content"
import type { Part } from "../../model/transcript"

/**
 * The session the sample plugin's widgets belong to (`widgets/fixture/`):
 * composition registers the plugin with it while the sample workspace is in
 * use, and its conversation carries one of each.
 */
export const sampleWidgetSession = "widget-hosts"

/**
 * The session the fixture MCP App's call belongs to (`widgets/app/fixture/`):
 * composition registers the app with it while the sample workspace is in
 * use, and its conversation carries the call's widget.
 */
export const sampleAppSession = "mcp-app-host"

const widget = (ref: WidgetRef): Part => ({ kind: "widget", widget: ref })

export const labsSamples: readonly SampleSession[] = [
  {
    id: "retry-budget",
    channelId: "gateway",
    title: "Retry budget for ACP reconnects",
    model: models.astra,
    status: "needs-you",
    updated: 3,
    started: 22,
    preview: "Wants to run the reconnect tests, including the new budget case.",
    now: "Waiting to run the reconnect tests after adding a token bucket",
    unread: true,
    approval: {
      id: "retry-tests",
      command: "cargo test -p nessa-gateway reconnect -- --nocapture",
      reason: "Runs the reconnect tests, including the new budget-exhaustion case.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
      ask: "tool",
    },
    messages: [
      [
        "user",
        22,
        [
          text(
            "Reconnects to the ACP harness hammer the gateway after a restart. Give them a retry budget.",
          ),
        ],
      ],
      // A long turn — dozens of steps over several messages — so the peek's
      // bound and its cost are seen and measured on something real.
      [
        "agent",
        18,
        [
          searched("reconnect · 11 results"),
          read("acp/reconnect.rs"),
          read("acp/client.rs"),
          read("acp/session.rs"),
          read("gateway/listener.rs"),
          read("gateway/accept.rs"),
          read("gateway/limits.rs"),
          searched("retry_interval · 6 results"),
          read("acp/config.rs"),
          read("acp/backoff.rs"),
          text(
            "Every client retries on a fixed 250 ms interval with no ceiling. After a gateway restart they all come back in the same few milliseconds, and the accept loop falls behind.",
          ),
        ],
      ],
      [
        "agent",
        12,
        [
          ran("cargo test -p nessa-gateway accept"),
          ran("cargo run -p nessa-gateway -- --simulate-clients 50"),
          read("target/storm.log"),
          searched("BudgetExhausted · 0 results"),
          read("acp/errors.rs"),
          edited("acp/errors.rs", 6),
          read("acp/tests/reconnect.rs"),
          edited("acp/tests/reconnect.rs", 38, 4),
          read("acp/tests/support.rs"),
          edited("acp/tests/support.rs", 12, 2),
          text(
            "Fifty simulated clients are enough to reproduce it: the accept queue peaks at 4,100 pending handshakes. I've added a failing test for a budget that runs out.",
          ),
          {
            kind: "code",
            code: "#[tokio::test]\nasync fn gives_up_once_the_budget_is_spent() {\n    let client = storm_client(Budget::new(5, secs(2)));\n    assert!(matches!(client.reconnect().await, Err(ReconnectError::BudgetExhausted)));\n}",
          },
        ],
      ],
      [
        "agent",
        4,
        [
          read("acp/reconnect.rs"),
          read("acp/backoff.rs"),
          edited("acp/backoff.rs", 22, 9),
          text(
            "The test fails as it should. I've given each client a token bucket — five attempts, one refilled every two seconds — and jittered the backoff:",
          ),
          {
            kind: "code",
            code: "let delay = backoff.next().with_jitter(0.2);\nif !budget.try_take() {\n    return Err(ReconnectError::BudgetExhausted);\n}",
          },
          edited("acp/reconnect.rs", 58, 14),
          {
            kind: "step",
            step: "edit",
            label: "Created",
            detail: "acp/budget.rs",
            added: 41,
          },
          edited("acp/mod.rs", 1),
          edited("acp/config.rs", 8, 3),
          read("docs/gateway.md"),
          edited("docs/gateway.md", 14, 2),
          ran("cargo check -p nessa-gateway"),
          ran("cargo clippy -p nessa-gateway -- -D warnings"),
          edited("acp/budget.rs", 3, 3),
          ran("cargo clippy -p nessa-gateway -- -D warnings"),
          text("I'd like to run the reconnect tests before calling it done."),
        ],
      ],
    ],
  },
  {
    id: "reconnect-storm",
    channelId: "gateway",
    title: "Reconnect storm after sleep",
    model: models.astra,
    status: "needs-you",
    updated: 12,
    started: 40,
    preview: "Asks to restart the local gateway to test jittered backoff.",
    now: "Waiting to restart the gateway with 200 clients to prove the jitter",
    approval: {
      id: "storm-restart",
      command: "cargo run -p nessa-gateway -- --simulate-clients 200",
      reason: "Restarts the local gateway and opens 200 connections.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
      ask: "tool",
    },
    messages: exchange(
      "After the Mac wakes, every client reconnects at once and the gateway drops half of them.",
      "They all retry on the same fixed schedule. I added full jitter to the backoff; to prove it I need to restart the local gateway with 200 simulated clients.",
      13,
    ),
  },
  {
    id: "command-order",
    channelId: "gateway",
    title: "Command drawn in order",
    model: models.sol,
    status: "needs-you",
    updated: 11,
    started: 20,
    preview: "Asks to send a note to the address written in the argument.",
    now: "Waiting to send, so the address can be read in the order it runs",
    approval: {
      id: "command-order-ask",
      command: `send ${JSON.stringify({ to: "\u202Emoc.live@bob\u202C" })}`,
      reason: "Sends the note to the address in the argument.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
      ask: "tool",
    },
    messages: exchange(
      "Send the note to that address.",
      "The address in the argument is not the one it looks like when the controls are hidden. I will not send it until it can be read in order.",
      11,
    ),
  },
  {
    id: "command-order-rtl",
    channelId: "gateway",
    title: "Command base direction",
    model: models.sol,
    status: "needs-you",
    updated: 10,
    started: 20,
    preview: "Asks to run a tool whose name is written right to left.",
    now: "Waiting to run it, so the tool stays left of its argument",
    approval: {
      id: "command-order-rtl-ask",
      command: `\u05E9\u05DC\u05D5\u05DD ${JSON.stringify({ to: "bob@example.com" })}`,
      reason: "Runs the tool named in the command, then its argument.",
      origin: { kind: "agent" },
      options: sampleApprovalOptions,
      ask: "tool",
    },
    messages: exchange(
      "Run that tool.",
      "The tool's name is written right to left. I will not run it until the name stays left of the argument.",
      10,
    ),
  },
  {
    id: "trace-ids",
    channelId: "gateway",
    title: "Trace IDs across the ACP boundary",
    model: models.fable,
    status: "running",
    updated: 6,
    started: 120,
    preview: "Threading the span context through session/prompt.",
    now: "Threading the span context through session/prompt as a _meta field",
    pinned: true,
    activity: ["Threading the span context", 212],
    messages: exchange(
      "I want one trace from the panel click to the agent's first token.",
      "Threading the span context through `session/prompt` as a `_meta` field, so the agent side can pick it up without a protocol change.",
      7,
    ),
  },
  {
    id: "backpressure",
    channelId: "gateway",
    title: "Backpressure on the event stream",
    model: models.opus,
    status: "running",
    updated: 8,
    started: 30,
    preview: "Bounding the per-client queue and dropping to snapshots when it fills.",
    now: "Running clippy on the bounded per-client queue before the stream tests",
    activity: ["Running cargo clippy", 95],
    messages: exchange(
      "A slow panel makes the event stream buffer without bound.",
      "Each client now gets a bounded queue; when it fills, the stream drops to a snapshot of the conversation instead of the backlog.",
      9,
      [searched("broadcast · 4 results"), edited("stream/fanout.rs", 36, 9)],
    ),
  },
  {
    id: "gateway-token",
    channelId: "gateway",
    title: "Rotate the gateway token",
    model: models.sonnet,
    updated: 190,
    preview: "The token now rotates on restart and the panel picks up the new one.",
    messages: exchange(
      "Rotate the gateway's token whenever it restarts.",
      "The token now rotates on restart, and the panel reads the new one from the keychain before reconnecting.",
      190,
    ),
  },
  {
    id: "rate-limit",
    channelId: "gateway",
    title: "Rate limit headers",
    model: models.pickle,
    updated: 5 * 24 * 60,
    preview: "Surfaces retry-after in the error the panel shows.",
    messages: exchange(
      "Show when a rate limit resets instead of a generic error.",
      "The gateway now forwards retry-after, and the panel says ‘Try again in 40 seconds’.",
      5 * 24 * 60,
    ),
  },
  {
    id: "gpt6-catalog",
    channelId: "sdk",
    title: "Model catalog for GPT-6",
    model: models.astra,
    status: "running",
    updated: 12,
    started: 20,
    preview: "Adding GPT-6 Astra with its context window and reasoning levels.",
    now: "Reading the catalog to add GPT-6 Astra with its reasoning levels",
    activity: ["Reading models.json", 212],
    messages: exchange(
      "Add GPT-6 to the model catalog.",
      "Adding GPT-6 Astra with its context window and reasoning levels; checking the composer reads it without a change.",
      13,
    ),
  },
  {
    id: "session-docs",
    channelId: "sdk",
    title: "Document session lifecycle",
    model: models.sonnet,
    updated: 27 * 60,
    preview: "Each public call now states what it does to a closed session.",
    messages: exchange(
      "The SDK docs never say what happens if you prompt a closed session.",
      "Every public call on Session now documents its behaviour after close, with a doctest for the error.",
      27 * 60,
    ),
  },
  {
    id: "quiet-chips",
    channelId: "design-system",
    title: "Quiet chips in the composer",
    model: models.opus,
    updated: 45,
    preview: "Chips rest as text in one colour and open frosted menus.",
    unread: true,
    messages: exchange(
      "The composer's chips shout. Make them quieter.",
      "They now rest as plain text in one colour, with no rim, and open frosted menus that spring into place.",
      45,
    ),
  },
  {
    id: "chip-focus",
    channelId: "design-system",
    title: "Chip focus ring",
    model: models.opus,
    updated: 7 * 24 * 60 - 60,
    preview: "Focus takes the theme's edge light instead of system blue.",
    messages: exchange(
      "The chips flash blue when focused. It fights the theme.",
      "Focus now uses a soft halo in the theme's edge light, the same colour as the resize glow.",
      7 * 24 * 60 - 60,
    ),
  },
  {
    id: sampleWidgetSession,
    channelId: "design-system",
    title: "Widget hosts, every state",
    model: models.opus,
    updated: 9 * 24 * 60,
    preview: "One widget of each kind a host can draw.",
    messages: [
      ["user", 9 * 24 * 60 + 4, [text("Show me each thing a widget host can draw.")]],
      [
        "agent",
        9 * 24 * 60,
        [
          text("The sample trail: open it beside us, or over the panes."),
          widget(sampleWidgets.trail),
          text("And what a host says when it cannot draw one:"),
          widget(sampleWidgets.unread),
          widget(sampleWidgets.missing),
          widget(sampleWidgets.off),
          widget(sampleWidgets.unshowable),
          widget(sampleWidgets.unregistered),
        ],
      ],
    ],
  },
  {
    id: sampleAppSession,
    channelId: "design-system",
    title: "An MCP App, in its sandbox",
    model: models.opus,
    updated: 9 * 24 * 60 + 30,
    preview: "The fixture server's app, drawn behind the sandbox proxy.",
    messages: [
      ["user", 9 * 24 * 60 + 34, [text("Show me the fixture app.")]],
      [
        "agent",
        9 * 24 * 60 + 30,
        [
          text("The fixture server's app, drawn in its sandbox:"),
          widget(fixtureWidget(sampleAppSession)),
        ],
      ],
    ],
  },
  {
    id: "detect-opencode",
    channelId: "onboarding",
    title: "Detect OpenCode on PATH",
    model: models.pickle,
    updated: 2 * 24 * 60,
    preview: "Setup now finds OpenCode wherever the shell would.",
    messages: exchange(
      "Setup says OpenCode isn't installed, but it is.",
      "Setup looked only in /usr/local/bin. It now resolves OpenCode the way the login shell would.",
      2 * 24 * 60,
    ),
  },
  {
    id: "intro-sound",
    channelId: "onboarding",
    title: "Intro sound volume",
    model: models.sol,
    updated: 8 * 24 * 60,
    preview: "Fades in over 1.2s and respects the system mute.",
    messages: exchange(
      "The intro sound is loud.",
      "It now fades in over 1.2 seconds.",
      8 * 24 * 60,
    ),
  },
  {
    id: "crdt",
    channelId: "reading-list",
    title: "Summarise the CRDT paper",
    model: models.opus,
    updated: 8 * 24 * 60 + 120,
    preview: "Five ideas worth stealing, and the one that doesn't apply to us.",
    messages: exchange(
      "Summarise ‘Peritext’ for someone who knows Yjs.",
      "Five ideas worth stealing, mostly around how marks expand at their boundaries, and one — tombstone compaction — that doesn't apply to a single-writer draft.",
      8 * 24 * 60 + 120,
    ),
  },
  {
    id: "nas-backup",
    channelId: "home-lab",
    title: "Back up the NAS to B2",
    model: models.sonnet,
    updated: 24 * 60 + 60,
    preview: "Nightly, encrypted, with a monthly restore test.",
    messages: exchange(
      "Back up the NAS to B2 every night.",
      "A nightly encrypted backup is in place, and a monthly job restores one folder to prove the backup can be read.",
      24 * 60 + 60,
    ),
  },
  {
    id: "calm-essay",
    channelId: "writing",
    title: "Essay: calm software",
    model: models.opus,
    updated: 3 * 24 * 60,
    preview: "A first draft, organised around three things calm software never does.",
    unread: true,
    messages: exchange(
      "Help me draft an essay on calm software.",
      "Here's a first draft, organised around three things calm software never does: interrupt, hurry, or hide what it is doing.",
      3 * 24 * 60,
    ),
  },
  {
    id: "zsh-xdg",
    channelId: "dotfiles",
    title: "Move zsh config to XDG",
    model: models.minimax,
    updated: 28 * 24 * 60,
    preview: "Everything under ~/.config/zsh with a one-line ~/.zshenv.",
    messages: exchange(
      "Tidy my home directory.",
      "Moved zsh to ~/.config/zsh; ~/.zshenv is one line now.",
      28 * 24 * 60,
    ),
  },
]
