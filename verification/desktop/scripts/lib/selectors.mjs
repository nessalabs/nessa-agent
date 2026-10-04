/**
 * Every way the verification scripts find or drive something on the desktop
 * page, in one place. When the UI changes, this is the one file to edit.
 *
 * Prefer roles, labels and stable `data-*` attributes; class names are used
 * only where the page offers nothing steadier, and are marked as such.
 */

/** What `[data-workspace]`'s data-content says is filling the content region. */
export const content = {
  panes: "panes",
  overview: "agents",
  widget: "widget",
}

const committedQuestions = "[data-committed-questions]"

/** CSS selectors, grouped by the part of the window they belong to. */
export const css = {
  // Committed conversation production-component fixture
  historyTabConsumer: "[data-history-tab-consumer]",
  committedFixture: "[data-committed-fixture]",
  committedControls: "[data-committed-controls]",
  committedQuestions,
  committedQuestionInputs: `${committedQuestions} input[type="radio"]`,
  committedLimitNotice: "[data-committed-limit-notice]",
  committedNotice: '[data-committed-controls] [role="status"]',
  committedActions: "[data-committed-controls] button",

  // The window
  surface: "[data-surface]",
  workspace: "[data-workspace]", // carries data-content (see `content`)
  anyReady: "[data-pane-key], [data-surface]",

  // The load fallback in index.html, before the frontend mounts
  loadMessage: "[data-nessa-load-message]",
  loadMark: "[data-nessa-load-mark]",
  loadTitle: "[data-nessa-load-title]",

  // Panes (data-pane-key is the pane's identity; data-pane-focused marks the focused one)
  pane: "[data-pane-key]",
  cornerPane: "[data-pane-key][data-split-corner]", // the top-left pane, whose header clears the window's controls when no side column is beside it
  panesAlone: "[data-workspace][data-panes-alone]", // both side columns closed: the corner pane's header steps past the controls
  focusedPane: "[data-pane-key][data-pane-focused]",
  paneGrid: "[data-split-grid]", // the grid the panes are laid out in — the box the drag measures
  paneHeader: ".workspace-pane-header", // class: a pane's header, which carries the drag
  paneDragHandle: "[data-drag-pane]",
  paneTitle: "[data-drag-pane] .workspace-pane-title", // class: where a pane is grabbed by its title
  titleText: ".workspace-pane-title", // class: a pane's title, in a pane or in the drag's copy of it
  paneBody: ".workspace-pane-body", // class: a pane's conversation and composer, below its header
  composer: "[data-pane-key] textarea",
  transcript: ".workspace-transcript", // class
  dock: ".workspace-dock", // class: a conversation's composer, at its pane's foot — and a new session's home's, docked there in a small pane
  conversationDock: ".workspace-conversation .workspace-dock", // class: a conversation's composer, never a home's
  paneHome: ".workspace-pane-home", // class: a new session's home in a pane
  homeScene: ".workspace-pane-home .desktop-header", // class: the home's night scene or picture
  greeting: ".workspace-pane-home .desktop-greeting", // class: "Working late?"
  conversationTitle: ".workspace-heading h2", // class: a conversation's title, atop its transcript
  composerCard: ".desktop-composer", // class: the composer's card
  homePage: ".desktop-home[data-page]", // class: a home whose long draft opened the page
  homeCustomize: ".workspace-pane-home .desktop-header-customize", // class: the scene's Customize control
  dragLayer: ".split-panes-layer", // class: what is carried is drawn in it, below the titlebar row
  dragGhost: ".split-panes-ghost", // class: the translucent copy the pointer carries
  dragCarrier: ".split-panes-carrier", // class: holds the copy at the pointer
  dragPlaceholder: ".split-panes-placeholder", // class: where a drop would land
  dragShield: ".split-panes-shield", // class: holds the pointer while carrying
  lifted: "[data-drag-lifted]",
  dragging: "[data-workspace][data-drag-carrying]",
  dropAnnouncer: '[data-workspace] [role="status"][aria-live="polite"]',

  // Widgets (ADR 326) and the sample plugin that shows them (src/desktop/widgets/fixture/)
  widgetCard: "[data-widget-inline]", // a widget's card in a message; its value says what it draws
  widgetPane: "[data-widget-pane]", // a pane showing a widget
  widgetBody: "[data-widget-body]", // a widget's body in a pane or the window, where its caret lands
  widgetWindow: "[data-widget-window]", // the window: a widget over the panes
  chatArea: ".workspace-chat", // class: the content region the panes, and the window, are drawn in
  widgetTrail: '[data-slot="breadcrumb"]', // a widget's way back, in its chrome
  sampleCard: "[data-sample-card]", // the sample trail's own card
  sampleView: "[data-sample-view]", // a sample widget's view; its value is the widget's id
  sampleStep: "[data-sample-step]", // a step of the trail, which opens its detail
  sampleDetail: "[data-sample-detail]", // the trail's open detail

  sampleAccessory: "[data-sample-accessory]", // the sample plugin's accessory in its session's header
  sampleSize: "[data-sample-size]", // the place's size as the sample view was told it, "<width>x<height>"

  // MCP Apps (ADR 344, #349) and the fixture app the sample workspace registers (src/desktop/widgets/app/fixture/)
  appFrame: "[data-app-frame]", // an app's sandbox proxy frame; its value is the place it is drawn in
  appFrameIn: (place) => `[data-app-frame="${place}"]`, // the app's frame in one place: inline, pane, window
  appView: "[data-app-view]", // an app's view; its value is the view's lifecycle
  appNotice: ".widget-app-notice", // class: a notice above a running app
  fixtureControl: (name) => `[data-fixture="${name}"]`, // a control inside the fixture app's own document
  fixtureState: (state) => `body[data-fixture-state="${state}"]`, // the fixture app saying where it is
  fixtureOutput: (name) => `#${name}`, // what the fixture app heard back: call, fetch, mode
  // The test MCP server's review app (scripts/mcp-test-server/server.mjs), as a real server serves it
  reviewControl: (name) => `[data-review="${name}"]`, // a control inside the review app: delete, fullscreen
  reviewState: (state) => `body[data-review-state="${state}"]`, // the review app saying where it is
  reviewOutput: (name) => `#${name}`, // what the review app heard back: result, first, hidden-no-ui, hidden-with-ui, again

  // Quick switcher (⌘K, ⌘\\)
  switcherField: '[role="dialog"] input', // the switcher's search field

  // Side columns
  sidebar: ".workspace-sidebar", // class
  sessionList: ".workspace-list", // class
  listSearch: ".workspace-list .workspace-search", // class: the session list's search field
  sessionRow: "[data-drag-item]", // a session a drag can carry to a pane
  sidebarEdge: '[role="separator"][aria-label="Resize Sidebar"]',

  // Approval card (arranged by its own width)
  approvalCard: ".workspace-approval", // class
  appApprovalCard: '.workspace-approval[data-origin="app"]', // class: a review an MCP App asked for, not the agent
  approvalActions: ".workspace-approval-actions button", // class
  approvalWord: ".workspace-approval-word", // class
  approvalHead: ".workspace-approval-head", // class: who asks, and what
  approvalHeadWords: ".workspace-approval-head-words", // class: the head's words, without its icon

  // Agents overview (always offered: the sidebar's entry and ⌘0)
  overviewEntry: ".workspace-sidebar .agents-overview-entry", // class: the sidebar's "Agents"
  overview: ".agents-overview", // class
  overviewItem: "[data-overview-item]",
  overviewRequest: ".agents-request", // class
  overviewRow: ".agents-row", // class
  overviewReplyPill: ".agents-reply-pill", // class
  overviewReplyField: "[data-reply-for] textarea", // data-reply-for is the session replied to
  overviewFilter: ".agents-filter", // class
  overviewHeader: ".agents-overview-header", // class: the title, its counts and the filter
  overviewScroll: ".agents-overview-scroll", // class: the list's scroller, under the header
  overviewCount: ".agents-overview-counts button", // class: a header count, a toggle (aria-pressed, data-group)
  overviewGroup: ".agents-overview-group", // class: a listed group, labelled by its h2
  overviewPeek: ".agents-overview-peek", // class: the peek beside the list, which scrolls
  peekSummary: ".agents-peek-summary", // class: what is going on, in the source's line
  peekStory: ".agents-peek-story", // class: the turn's story, top to bottom
  peekEarlier: ".agents-peek-earlier", // class: says the turn holds more above what the peek draws
  transcriptStep: ".workspace-steps li", // class: one step the agent took, in a message
  overviewColumn: ".agents-overview-column", // class: the list the arrow keys walk
  overviewResting: ".agents-overview-resting", // class: a quiet line in the list ("Nothing needs you")
  inlinePeek: ".agents-inline-peek", // class: the peek opened beneath its row
  overviewTitle: ".agents-overview-title", // class: the overview's heading and filter
  peekAsk: ".agents-peek-ask", // class: the request in full, after the story

  // The composer's thinking control (src/desktop/ui/thinking-control.tsx): a chip
  // opening a popover holding a slider; the levels are read from
  // the page, never named here (their owner is src/desktop/model/composer-options.ts)
  composerForm: ".desktop-composer", // class: a composer, whose place must not move
  thinkingChip: 'button[aria-haspopup="dialog"][aria-label^="Thinking level"]',
  thinkingPopover: '[role="dialog"][aria-label="Thinking"]',
  modelChip: '[data-slot="model-picker-trigger"]', // the composer's model picker
  modelPicker: '[data-slot="model-picker-content"]', // its list, a model each role="option"
  modelOption: '.desktop-model-picker [role="option"]', // class: a model in the picker
  modelProviderTab: '.desktop-model-picker [role="tab"]', // class: a provider's tab in the picker
  thinkingSlider: '[role="dialog"][aria-label="Thinking"] [role="slider"]',
  thinkingTrack: ".desktop-thinking-slider", // class: the slider's pointer room; data-ultra when the model has Ultra
  thinkingFast: '[role="dialog"][aria-label="Thinking"] button[aria-label="Fast mode"]',
  thinkingLeaving: ".desktop-thinking-words[data-leaving]", // class: the words that were shown

  // Column heads (src/desktop/ui/column-header.tsx): data-title / data-placement are "inline" | "below"
  columnBar: ".desktop-column-bar", // class
  columnTitle: ".desktop-column-title[data-placement]:not(.desktop-column-sizer)", // class
  titlebarButtons: ".workspace-titlebar button, .settings-titlebar button", // class

  // Settings (data-sidebar is "open" | "closed"; --settings-sidebar-w its width)
  settings: ".settings", // class
  settingsTitlebar: ".settings-titlebar", // class
  settingsSidebarToggle: '.settings-titlebar [aria-controls="settings-sidebar"]', // Settings' own sidebar toggle
  settingsCategory: ".settings-nav-item", // class: a category in Settings' sidebar
  settingsHeading: "#settings-heading", // the open category's name
  settingsTab: '.settings-tabs [role="tab"]',
  settingsPanel: "#settings-panel", // the open tab's page
  control: 'button, input, select, textarea, [role="switch"]',

  // Elements by kind, inside a part found by one of the above
  field: "textarea",
  button: "button",
  heading: "h1, h2, h3",

  // Menus and tooltips
  menu: '[role="menu"]',
  menuItem: '[role^="menuitem"]',
  paneActions: '[aria-label^="Pane Actions"]', // label: a pane's "…" menu, in its header
  sliverPicture: "[data-sliver] img", // a conversation pane's band showing the header picture, not the scene
  paneRefusal: '.workspace-pane-header [role="status"]', // why a picture chosen from a pane's menu was not taken
  tooltip: ".desktop-tooltip", // class

  // Picture band atop conversation panes (decorative, aria-hidden)
  pictureBand: "[data-sliver]",
}

/**
 * What may sit under the window's controls: titlebar rows themselves (their
 * content starts at the safe area by the stylesheet's rule) and floating
 * layers. Nothing else — the drag's copy included, which its layer clips
 * below the titlebar row — ADR 238, "The titlebar's safe area": nothing is
 * painted under the controls, decorative art (the picture band, the night
 * scene) included.
 */
export const safeAreaExempt = [
  ".workspace-titlebar",
  ".settings-titlebar",
  "[data-slot='app-shell-titlebar']",
  ".desktop-tooltip",
  ".desktop-column-sizer",
].join(", ")

/** The CSS custom properties the safe area is read from. */
export const safeAreaTokens = {
  start: "--desktop-titlebar-safe-start",
  height: "--desktop-titlebar-height",
}

/**
 * Model modules a script reads a rule from rather than retyping it (gate 13),
 * imported in the page through the dev server — so only with `--mode dev`.
 */
export const modules = {
  drop: "/src/desktop/split-panes/model/drop.ts",
  /** What a widget host says in each case (`hostLines`). */
  hostTable: "/src/desktop/widgets/model/host-table.ts",
}

/** The SDK's model catalogue, which the composer reads; scripts read it too, never retype it. */
export const catalogueFile = new URL(
  "../../../../crates/nessa-sdk/data/models.json",
  import.meta.url,
)

/** localStorage keys the scripts seed before the page loads. */
export const storage = {
  layout: "nessa.desktop.workspace-layout",
  motion: "nessa.desktop.motion",
  theme: "nessa.desktop.theme",
  pictureInConversations: "nessa.desktop.picture-in-conversations",
  greeting: "nessa.desktop.greeting",
}

/** The same-window event a stored preference announces a change on. */
export const preferenceEvents = {
  layout: "nessa:workspace-layout",
  greeting: "nessa:desktop-greeting",
}

/** The workspace layouts the scripts cover (`src/desktop/model/workspace-layout.ts`). */
export const layouts = ["columns", "sidebar"]

/**
 * Keyboard chords, as Playwright names them
 * (`src/desktop/workspace/ui/layouts/shortcuts.ts` is their owner).
 */
export const keys = {
  escape: "Escape",
  enter: "Enter",
  down: "ArrowDown",
  up: "ArrowUp",
  left: "ArrowLeft",
  right: "ArrowRight",
  home: "Home",
  end: "End",
  toggleSidebar: "Meta+KeyB",
  toggleSessionList: "Meta+Alt+KeyS",
  newSession: "Meta+KeyN",
  newSessionBeside: "Meta+Shift+KeyN",
  closePane: "Meta+KeyW",
  openBeside: "Meta+Backslash",
  splitDown: "Meta+Shift+Backslash",
  switcher: "Meta+KeyK",
  focusPane: (n) => `Meta+Digit${n}`,
  focusPrevious: "Meta+Shift+BracketLeft",
  focusNext: "Meta+Shift+BracketRight",
  moveLeft: "Control+Alt+ArrowLeft",
  moveRight: "Control+Alt+ArrowRight",
  overview: "Meta+Digit0",
  settings: "Meta+Comma",
  allow: "Meta+Enter",
  /** ⌘↩ as its two keys, for holding it down (`keyboard.down` repeats). */
  command: "Meta",
  deny: "Meta+Backspace",
  reply: "Meta+KeyR",
  /**
   * To the next control, a button included, by engine: WebKit, as Safari and
   * WKWebView on macOS, moves Tab through fields only unless ⌥ is held.
   */
  nextControl: { chromium: "Tab", webkit: "Alt+Tab" },
}

/**
 * How the drop announcer says a zone (`saying` in
 * src/desktop/split-panes/adapters/dom/drag.ts): a vertical zone is "above" or
 * "below" the pane — never "above of" — a side zone "left of" or "right of".
 */
export const zoneSaid = {
  any: /./,
  swap: /^Swap with /,
  vertical: /^(Move|Split) (above|below) (?!of )/,
  side: /^(Move|Split) (left|right) of /,
}

/** Accessible names, for getByRole / getByText. */
export const names = {
  agentsEntry: "Agents",
  /** An app's review card's head: the app by its server, and the tool it asked to run (`approvalHead`). */
  appAsks: (server, tool) => `The ${server} app wants to run ${tool}`,
  /** A pane's "…" menu: the header picture's two choices (issue #320). */
  chooseHeaderPicture: "Choose Header Picture…",
  useNightScene: "Use Night Scene",
  allowOnce: "Allow Once",
  leaveSettings: "Back to nessa Agent",
  /** A sample session (in-memory source) that waits on an approval. */
  approvalSession: "Release build signing",
  /** Sample sessions (in-memory source) that each wait on one approval. */
  approvalSessions: ["Release build signing", "Notarize the macOS", "Reconnect storm"],
  /** A sample session (in-memory source) waiting on an approval after a long turn, by its id (data-overview-item). */
  storySessionId: "retry-budget",
  denyOnce: "Deny",
  /**
   * Models the thinking check switches between, by what the SDK catalogue
   * (`catalogueFile`) publishes for them; the check confirms each there before
   * judging the page. No catalogue model publishes a level past Max today, so
   * none is named for Ultra: the check holds that none draws it.
   */
  modelWithFast: "Claude Opus 5",
  modelWithoutFast: "Claude Sonnet 5",
  /** Reasons, with no effort level recorded: the thinking chip is disabled. */
  modelWithoutLevels: "Claude Haiku 4.5",
  /** Its least level is None, which `modelWithoutFast` does not offer. */
  modelWithLeastLevel: "GPT-5.6 Sol",
  alwaysAllow: "Always Allow",
  /** The sample session (in-memory source) whose conversation carries a widget of each state. */
  widgetSession: "Widget hosts, every state",
  /** The channel the sample session is in. */
  widgetChannel: "design-system",
  /** The sample session (in-memory source) whose conversation carries the fixture MCP App's call. */
  appSession: "An MCP App, in its sandbox",
  /** What the fixture server refuses its hidden tool with (`fixture-plugin.ts`). */
  hiddenToolRefused: "fixture_secret is not available to apps",
  /**
   * What an app is told of a refused call to a real server through the
   * gateway (`widgets/app/adapters/gateway/mcp-app-server.ts`), by why.
   */
  gatewayRefused: {
    notForApp: "This app may not use that tool",
    declined: "The person declined this action",
    withdrawn: "The request was withdrawn",
  },
  /** What the fixture app says on its body (`fixture-app.ts`), by the field it says it in. */
  fixtureSays: {
    state: "data-fixture-state",
    mode: "data-fixture-mode",
    input: "data-fixture-input",
    result: "data-fixture-result",
    context: "data-fixture-context",
  },
  /** A channel's "Show all" in the sidebar, behind which its older sessions are. */
  showAll: /^Show all \d+$/,
  /** The line for an app the host cannot show (`app-view.ts`, `appLines.load`). */
  appLoadLine: "This app couldn't be loaded",
  /** The notice above an app whose CSP blocked a load (`app-view.ts`, `appLines`). */
  blockedNotice: "Blocked a connection this app didn't declare: https://example.com",
  /** Every notice the host may put above an app (`appLines`): its words, and an origin. */
  appNotices: [
    /^This app's server has stopped$/,
    /^Blocked a connection this app didn't declare(: [a-z]+:\/\/[^\s,]+(, [a-z]+:\/\/[^\s,]+)*)?$/,
  ],
  openWidget: "Open",
  openInWindow: "Open in Window",
  closePane: "Close Pane",
  closeWindow: "Close",
}

/** Role and accessible-name selectors for the committed conversation fixture. */
export const committedRoles = {
  state: (state, authority) => [
    "button",
    { name: authority ? state : "complete without live attachment", exact: true },
  ],
  questions: (limited) => [
    "button",
    { name: limited ? "questions with display limit" : "questions", exact: true },
  ],
  tabHistory: (state, truncated) => [
    "button",
    { name: `tabs:${state}${truncated ? ":truncated" : ""}`, exact: true },
  ],
  closeHistory: ["button", { name: "Close history tab", exact: true }],
}

/** Console noise that is known to be harmless (see CHECKLIST.md, "Console errors"). */
export const harmlessConsole = [
  // A fresh browser asks for /favicon.ico, which the dev server does not serve.
  // Chrome's message does not name the URL, so both the text and the source are matched.
  { text: /status of 404/i, url: /\/favicon\.ico(\?|$)/ },
  // The fixture MCP App asks for a page its CSP does not declare, on purpose
  // (`mcp-apps.mjs --only csp`): the engine reports the refusal it is checked for.
  // Its navigations of its own frame are refused by the proxy's policy, and
  // reported against the proxy's page (`--only escape-navigate,escape-refresh,escape-rewrite,departures`).
  {
    text: /Content Security Policy|Refused to (connect|frame)/i,
    url: /^(about:srcdoc|https?:\/\/127\.0\.0\.1:\d+\/proxy\.html)?$/,
  },
]

/** Actual-panel attachment race verification gestures and refusal notices. */
export const attachmentVerification = {
  editor: '[contenteditable="true"]',
  addAttachment: ["button", { name: "Add attachment", exact: true }],
  addFiles: ["button", { name: "Add files", exact: true }],
  selectedName: "picked.png",
  pendingNotice: "Attachments still loading",
  busyDropNotice: "Still reading files",
}

/** Setup request-deadline verification fixture and user-facing controls. */
export const readinessVerification = {
  checkingButton: "Checking…",
  retryButton: "Check again",
  readyObservation: "false:ready",
  buttons: "button",
}

/**
 * The app-review fixture (`fixtures/app-review/`, #436): the window over a
 * fake gateway whose one conversation holds an MCP App's call, which asks for
 * a review when the page calls a tool (`__appReview.call`). The longest
 * tool's name is the page's (`__appReview.longestTool`), from the client's
 * own bound.
 */
export const appReview = {
  page: "verification/desktop/fixtures/app-review/index.html",
  session: "Clean up the stale rows",
  sessionId: "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d",
  /** The card's head: the app by its server, and the tool it named. */
  head: (tool) => names.appAsks("mcptest", tool),
  /** The overview row's accessible name: the title, then the app asking. */
  row: (tool) => `Clean up the stale rows. ${names.appAsks("mcptest", tool)} {}.`,
  tool: "app_delete_row",
  /** The fixture page's title, by which the script knows it is served. */
  title: "Nessa: an app's review",
}
