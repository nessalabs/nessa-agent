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
  ambientGrain: ".desktop-grain", // class: the tiled, baked noise over the ambient light
  surface: "[data-surface]",
  workspace: "[data-workspace]", // carries data-content (see `content`)
  anyReady: "[data-pane-key], [data-surface]",

  // The load fallback in index.html, before the frontend mounts.
  // loadMark and startupMark must match nothing: the screens do not paint the avatar.
  loadMessage: "[data-nessa-load-message]",
  loadMark: "[data-nessa-load-mark]",
  loadTitle: "[data-nessa-load-title]",
  startupScreen: "[data-nessa-startup-screen]",
  startupMark: "[data-nessa-startup-mark]",
  startupLine: "[data-nessa-startup-line]",
  startupCode: "[data-nessa-startup-code]",
  startupRestart: '[data-nessa-startup-action="restart"]',
  startupQuit: '[data-nessa-startup-action="quit"]',

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
  message: ".workspace-message[data-role]", // class: one message in a transcript; data-role is user or agent
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
  dragTitle: ".workspace-drag-title", // class: the compact carried title
  dragGhost: ".split-panes-ghost", // class: the opaque copy the pointer carries
  dragCarrier: ".split-panes-carrier", // class: holds the copy at the pointer
  dragPlaceholder: ".split-panes-placeholder", // class: where a drop would land
  dragShield: ".split-panes-shield", // class: holds the pointer while carrying
  lifted: "[data-drag-lifted]",
  dragging: "[data-workspace][data-drag-carrying], [data-workspace][data-drag-card]",
  dropAnnouncer: '[data-workspace] [role="status"][aria-live="polite"]',

  // Widgets (ADR 326) and the sample plugin that shows them (src/desktop/widgets/fixture/)
  widgetCard: "[data-widget-inline]", // a widget's card in a message; its value says what it draws
  widgetPane: "[data-widget-pane]", // a pane showing a widget
  widgetBody: "[data-widget-body]", // a widget's body in a pane or the window, where its caret lands
  widgetWindow: "[data-widget-window]", // the window: a widget over the panes
  chatArea: ".workspace-chat", // class: the content region the panes, and the window, are drawn in
  workspaceEmpty: '.workspace-empty[role="status"]', // class: why the workspace has nothing to show
  workspaceEmptyText: '.workspace-empty[role="status"] [data-slot="empty-state-title"]', // class: its sentence
  workspaceEmptyRetry: '.workspace-empty[role="status"] button', // class: its Try Again
  transcriptNote: '.workspace-transcript-note[role="status"]', // class: why a shown conversation could not be read
  unreadablePart: "[data-unreadable-position]", // a saved part this build could not read, drawn in place
  transcriptNoteText: ".workspace-transcript-note p", // class: that note's sentence
  peekFailure: '.agents-peek-failure[role="status"]', // class: a peek's sentence — a conversation it could not read, or an answer it could not confirm
  widgetTrail: '[data-slot="breadcrumb"]', // a widget's way back, in its chrome
  sampleCard: "[data-sample-card]", // the sample trail's own card
  sampleView: "[data-sample-view]", // a sample widget's view; its value is the widget's id
  sampleStep: "[data-sample-step]", // a step of the trail, which opens its detail
  sampleDetail: "[data-sample-detail]", // the trail's open detail

  sampleAccessory: "[data-sample-accessory]", // the sample plugin's accessory in its session's header
  sampleSize: "[data-sample-size]", // the place's size as the sample view was told it, "<width>x<height>"
  subagentPanel: "[data-subagent-panel]", // the subagents widget's view
  subagentList: "[data-subagent-list]", // its list of children
  subagentRow: "[data-subagent-row]", // one child; data-subagent-name is the child's name
  subagentDetail: "[data-subagent-detail]", // the open child's conversation
  subagentScroll: "[data-subagent-scroll]", // the conversation's scroller
  subagentMessages: "[data-subagent-messages]", // the messages that scroller follows
  subagentSummary: "[data-subagent-summary]", // the list's counts
  subagentStack: "[data-subagent-stack]", // the conversation's subagents, in its pane header
  avatarFace: '[data-slot="random-avatar-paint"]', // one face; aria-label is its name
  avatarMore: '[data-slot="avatar-stack-more"]', // the count of faces the stack does not show

  // MCP Apps (ADR 344, #349) and the fixture app the sample workspace registers (src/desktop/widgets/app/fixture/)
  appFrame: "[data-app-frame]", // an app's sandbox proxy frame; its value is the place it is drawn in
  appView: "[data-app-view]", // an app's view; its value is the view's lifecycle
  appNotice: ".widget-app-notice", // class: a notice above a running app
  // The test MCP server's review app (scripts/mcp-test-server/server.mjs), as a real server serves it
  chartApp: "#chart", // the test server's chart app: the series it drew from its tool result

  // Quick switcher (⌘K, ⌘\\)
  switcherField: '[role="dialog"] input', // the switcher's search field
  switcherResults: "#workspace-switcher-results", // the switcher's listbox

  // Shared controls (#632): the kit's components, by the slot each draws
  keyCap: "kbd", // every key cap; each must be the kit's (data-slot="kbd")
  kitRow: '[data-slot="sidebar-menu-item-row"] > [data-size]', // the kit's SidebarMenuItem control (data-size is always written; a context menu's trigger replaces data-slot)
  kitRowFrame: '[data-slot="sidebar-menu-item-row"]', // the kit's row around its control and what it lays beside it
  rowLabel: '[data-slot="sidebar-menu-item-label"], .desktop-list-row-label', // class: a row's words, in either row component
  listRow: ".desktop-list-row", // class: the window's ListRow (src/desktop/ui/list-row.tsx)
  segmentedControl: '[data-slot="segmented-control"]', // the kit's SegmentedControl
  emptyTitle: '[data-slot="empty-state-title"]', // the kit's EmptyState's words
  identityRow: ".desktop-identity", // class: the row holding the identity
  identityProduct: ".desktop-identity-words > span", // class: the identity's word after "nessa"
  identityButton: ".desktop-identity-button", // class: "nessa Studio" / "‹ nessa Agent" (src/desktop/ui/identity.tsx)
  emptyState: '[data-slot="empty-state"]', // the kit's EmptyState
  countBadge: ".workspace-badge", // class: a sidebar row's count, the kit's Badge
  litPoint: '.workspace-status:is([data-status="needs-you"], [data-status="unread"])', // class: StatusGlyph's lit points
  needsYouHeading: "#agents-needs-you", // the overview's Needs you heading
  codeBlock: ".workspace-code", // class: a code block in a transcript
  allClear: ".agents-clear", // class: the overview's "Nothing needs you" card

  // Side columns
  sidebar: ".workspace-sidebar", // class
  sessionList: ".workspace-list", // class
  chat: ".workspace-chat", // class: the chat area, the panes' column
  sideRail: ".side-rail", // class: the rail beside the workspace
  workspaceWindow: ".workspace-window", // class: the frame holding the rail and the workspace; data-rail, data-sidebar
  titlebarSidebarToggle: '.workspace-titlebar [aria-controls="workspace-sidebar"]', // the titlebar's Show/Hide Sidebar
  sideRailToggle: ".side-rail-toggle", // class: shows or hides the rail; aria-expanded
  studio: ".workspace-identity .desktop-identity-button", // class: "nessa Studio", which opens Settings
  peekEdge: ".desktop-peek-edge", // class: the strip at the folded sidebar's edge a hover reveals it from
  listSearch: ".workspace-list .workspace-search", // class: the session list's search field
  listScroll: ".workspace-list-scroll", // class: the session list's scroller
  sessionListRow: ".workspace-list [data-session-row]", // a row of the session list, not a sidebar thread
  sessionRow: "[data-drag-item]", // a session a drag can carry to a pane
  sidebarEdge: '[role="separator"][aria-label="Resize Sidebar"]',

  // Approval card (arranged by its own width)
  approvalCard: ".workspace-approval", // class
  appApprovalCard: '.workspace-approval[data-origin="app"]', // class: a review an MCP App asked for, not the agent
  agentApproval: '.workspace-approval[data-origin="agent"]', // class: a review the agent asked for, not an app
  approvalReason: ".workspace-approval-reason", // class: why the review is asking
  approvalActions: ".workspace-approval-actions button", // class
  approvalWord: ".workspace-approval-word", // class
  approvalHead: ".workspace-approval-head", // class: who asks, and what
  approvalHeadWords: ".workspace-approval-head-words", // class: the head's words, without its icon
  approvalCommand: ".workspace-approval-command", // class: what is asked, whole: a command, or an app's tool and its message

  // A message of the person's that an MCP App wrote (#390)
  messageAuthor: ".workspace-message-author", // class: which app wrote it, above its bubble
  bubble: ".workspace-bubble", // class: a message of the person's

  // Agents overview (always offered: the sidebar's entry and ⌘0)
  overviewEntry: ".workspace-sidebar .agents-overview-entry", // class: the sidebar's "Agents"
  overview: ".agents-overview", // class
  overviewItem: "[data-overview-item]",
  // On the column once this open has drawn every row (`overview.tsx`).
  overviewListed: "[data-overview-listed]",
  overviewRequest: ".agents-request", // class
  overviewCommand: ".agents-request-command", // class: the command on a request row
  overviewRequestActions: ".agents-request-actions", // class: a request row's answers, at its end while pointed at or the keyboard is visibly on it
  overviewRowItem: ".agents-row-item", // class: a listed row's item
  overviewEdge: ".agents-overview-edge", // class: the resize edge between the list and the peek beside it (role=separator)
  overviewSide: ".agents-overview-side", // class: the list's side of the overview, whose right is the edge
  overviewSurface: ".agents-overview-surface", // class: the overview's card, holding the list and the peek
  overviewSaid: ".agents-overview-said", // class: the overview's live region, which says what became of an answer
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
  peekCommand: ".agents-peek .workspace-approval-command", // class: the command in the peek, beside the list or under the row
  peekStory: ".agents-peek-story", // class: the turn's story, top to bottom
  peekEarlier: ".agents-peek-earlier", // class: says the turn holds more above what the peek draws
  transcriptStep: ".workspace-steps li", // class: one step the agent took, in a message
  overviewColumn: ".agents-overview-column", // class: the list the arrow keys walk
  overviewResting: ".agents-overview-resting", // class: a group shown alone that lists nothing ("Nothing needs you"), the kit's EmptyState
  overviewFootnote: ".agents-overview-footnote", // class: how many sessions the view leaves out, and Show All
  inlinePeek: ".agents-inline-peek", // class: the peek opened beneath its row
  overviewTitle: ".agents-overview-title", // class: the overview's heading and filter
  peekAsk: ".agents-peek-ask", // class: the request in full, after the story
  overviewPeekScroll: ".agents-overview-peek .agents-peek-scroll", // class: what the peek beside the list says, scrolling above its reply pill

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
  settingsCard: '[data-slot="settings-group-card"]', // the UI kit's card on a Settings page, which what it holds must lie inside
  settingsFormCard: ".settings-card", // class: a card Settings draws itself (Integrations' form and inspection), there only while open
  settingsGroupHeading: ".settings-group > h2", // class: a group's heading on a Settings page
  settingsScrollers: ".settings-content, .settings-scroll, .settings-panel", // class: what scrolls in Settings, which must not scroll sideways
  settingsScroll: ".settings-scroll", // class: the page's scroller, below its bar
  settingsBar: ".settings-bar", // class: the page's titlebar row (drag strip); data-condensed on .settings-content once the masthead's title has scrolled under it
  settingsBarTitle: ".settings-bar-title", // class: the page's name, small, in that row
  settingsContent: ".settings-content", // class
  settingsMasthead: ".settings-masthead", // class: the page's head: title, dek, tabs
  settingsDek: ".settings-dek", // class: the one line under a title, where a category has one
  settingsRow: '[data-slot="settings-row"]', // the UI kit's row, on any Settings page
  settingsFound: "[data-setting][data-found]", // where a search just landed
  settingsSearch: 'input[aria-label="Search settings"]',
  control: 'button, input, select, textarea, [role="switch"]',

  // Settings › Connections › Integrations: the gateway's MCP servers
  // (src/desktop/settings/ui/integrations-tab.tsx)
  mcpGroup: '[data-setting="mcp-servers"]', // the servers' card; data-pending with no gateway
  // Settings › Connections › Linked devices (src/desktop/settings/ui/linked-devices-tab.tsx)
  linked: "[data-linked]", // the tab; its value: pending | checking | off | on | refused | unreachable
  linkedOn: '[data-linked="on"]',
  linkedOff: '[data-linked="off"]',
  linkedRefused: '[data-linked="refused"]',
  linkedPending: '[data-linked="pending"]',
  linkedNotice: "[data-linked-notice]",
  pairingCode: '[data-slot="pairing-code"]',
  signalOrb: '[data-slot="signal-orb"]',
  qrOrb: '[data-slot="qr-orb"]',
  fingerprint: '[data-slot="key-fingerprint"]',
  mcpServers: "[data-mcp-servers]", // the tab; its value: loading | listed | not-configured | failed | too-large | not-admin
  mcpRow: "[data-mcp-server]", // a server's row; its value is the server's name
  mcpStoredRow: "[data-mcp-server]:not([data-managed])",
  mcpAnyGroup: "[data-mcp-group]", // any name stored more than once
  mcpShared: "[data-mcp-shared]", // a group's "N servers share this name…"
  mcpSharedRow: "[data-mcp-shared-row]", // one read-only server of a group
  mcpManagedRow: "[data-mcp-server][data-managed]",
  mcpRowText: 'div:has(> [data-slot="settings-row-label"])', // a row's name with its command and variables (the kit's row's text column)
  mcpRowActions: ".settings-server-actions", // class: a row's buttons and switch
  mcpEmpty: "[data-mcp-empty]",
  mcpTooLarge: "[data-mcp-too-large]", // a list too large to show: its sentence and the remove-by-name field (U44)
  mcpNotices: "[data-mcp-notices]", // the notices' live region, drawn from the tab's first draw
  mcpNotice: "[data-mcp-notice]", // what an answer said, in the notices' region
  mcpConfirm: "[data-mcp-confirm]", // a row's "Remove …?"
  mcpForm: "[data-mcp-form]", // the add or edit form; its value is the stored name edited, empty while adding
  mcpVariable: "[data-mcp-variable]", // a variable's row in the form
  mcpVariableAdded: "[data-mcp-variable-key]", // a variable's row whose name is typed in the form
  mcpSecret: "[data-mcp-secret]", // a variable's value: a password field, uncontrolled
  mcpSecretHeld: "[data-mcp-secret-held]", // a value pasted with line breaks, held and not drawn (S4)
  mcpPasted: "[data-mcp-pasted]", // its "Pasted value: N lines"
  mcpArgument: "[data-mcp-argument]", // an argument's row in the form; its value is its place
  mcpValuesNeeded: "[data-mcp-values-needed]", // why Save waits on stored values, empty until it does
  mcpProblem: "[data-mcp-problem]", // a field's problem region, empty until a refusal; its value is the field
  mcpInspection: "[data-mcp-inspection]", // the inspection panel; its value: running | done | failed
  mcpInspectionHeading: "[data-mcp-inspection] h2", // the inspection's heading, where focus lands
  mcpInspectionStatus: "[data-mcp-inspection-status]", // what the inspection says of how it ended
  mcpAnyTool: "[data-mcp-tool]", // any inspected tool
  mcpCut: "[data-mcp-cut]",
  mcpSwitch: '[role="switch"]',

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

/** Parameterized selectors, built before passing their strings to the page. */
export const selectorFor = {
  railItem: (id) => `[data-rail-item="${id}"]`, // a place on the side rail: agents, notes, …
  appFrameIn: (place) => `[data-app-frame="${place}"]`, // the app's frame in one place: inline, pane, window
  fixtureControl: (name) => `[data-fixture="${name}"]`, // a control inside the fixture app's own document
  fixtureState: (state) => `body[data-fixture-state="${state}"]`, // the fixture app saying where it is
  fixtureOutput: (name) => `#${name}`, // what the fixture app heard back: call, fetch, mode, message, context
  reviewControl: (name) => `[data-review="${name}"]`, // a control inside the review app: delete, fullscreen
  reviewState: (state) => `body[data-review-state="${state}"]`, // the review app saying where it is
  chartState: (state) => `body[data-chart-state="${state}"]`, // the chart app saying where it is
  reviewOutput: (name) => `#${name}`, // what the review app heard back: result, first, hidden-no-ui, hidden-with-ui, again
  linkedAction: (action) => `[data-linked-action="${action}"]`,
  mcpServersIn: (phase) => `[data-mcp-servers="${phase}"]`,
  mcpRowNamed: (name) => `[data-mcp-server="${name}"]`,
  mcpGroupNamed: (name) => `[data-mcp-group="${name}"]`, // a name stored more than once: its read-only rows and one action (G3)
  mcpVariableNamed: (name) => `[data-mcp-variable="${name}"]`, // a stored variable's row, by its name
  mcpField: (field) => `[data-mcp-field="${field}"]`, // a form field by name: command
  mcpProblemFor: (field) => `[data-mcp-problem="${field}"]`, // a field's problem region by field: form for the form's own
  mcpAction: (action) => `[data-mcp-action="${action}"]`, // a tab's control by what it does: add | edit | inspect | remove | removeFirst | cancel | confirm | close | removeByName (the name field) | clear-value | trim-value | keep-value
  mcpInspectionIn: (phase) => `[data-mcp-inspection="${phase}"]`,
  mcpTool: (name) => `[data-mcp-tool="${name}"]`, // an inspected tool
  mcpBadge: (badge) => `[data-badge="${badge}"]`, // a tool's badge: read-only | destructive | ui
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
  /** The gateway source's poll and reconnect timing (`defaultGatewayTiming`). */
  gatewaySource: "/src/desktop/workspace/adapters/gateway/gateway-source.ts",
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
  sideRail: "nessa.desktop.side-rail", // "on" offers the side rail (Advanced › Experimental)
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
const chordModifiers = ["Alt", "Control", "Meta", "Shift"]

/**
 * The key and the modifiers a Playwright chord holds down. Every other
 * modifier is up. The last segment is the `KeyboardEvent.code`. The page
 * listener that proves a chord arrived matches this, so it does not spell
 * the chord a second time.
 */
export function chordDown(chord) {
  const parts = String(chord).split("+")
  const code = parts.at(-1) ?? ""
  const modifiers = parts.slice(0, -1)
  if (!code || modifiers.some((name) => !chordModifiers.includes(name)))
    throw new Error(`not a Playwright chord: ${chord}`)
  return {
    code,
    altKey: modifiers.includes("Alt"),
    ctrlKey: modifiers.includes("Control"),
    metaKey: modifiers.includes("Meta"),
    shiftKey: modifiers.includes("Shift"),
  }
}

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
  /** An app's message's review card's head and overview row: what it asks (`approvalHead`, #390). */
  appAsksToMessage: (server) => `The ${server} app wants to send a message as you`,
  /** The label above a message an app wrote (`messageAuthor`, #390). */
  sentBy: (tool, server) => `Sent by ${tool}, from ${server}`,
  /** A pane's "…" menu: the header picture's two choices (issue #320). */
  chooseHeaderPicture: "Choose Header Picture…",
  useNightScene: "Use Night Scene",
  allowOnce: "Allow Once",
  leaveSettings: "Back to nessa Agent",
  /** Settings' category and tab holding the MCP servers. */
  connections: "Connections",
  integrations: "Integrations",
  linkedDevices: "Linked devices",
  /** What Integrations says (`sentences` in src/desktop/settings/model/mcp-servers.ts). */
  mcp: {
    add: "Add server…",
    addVariable: "Add variable",
    save: "Save",
    cancel: "Cancel",
    edit: "Edit",
    inspect: "Inspect",
    remove: "Remove",
    close: "Close",
    name: "Name",
    command: "Command",
    addArgument: "Add argument",
    argument: (place) => `Argument ${place}`,
    variableName: "Variable name",
    storedValue: "Stored value kept",
    storedValueAgain: "Enter the value again",
    valuesAgain:
      "Changing the command, arguments or variables needs every stored value entered again.",
    empty: "No servers yet",
    notAdmin: "Only an administrator can manage MCP servers.",
    conflict: "Changed elsewhere, the list was reloaded. Check and try again.",
    gone: (name) => `“${name}” is no longer stored.`,
    listUnreadable:
      "The servers couldn't be listed: the configuration file can't be read as it is.",
    listTooLarge:
      "The server list is too large to show. Removing a server fixes it: enter its name.",
    saveTooLarge:
      "This would make the server list too large; remove a server or shorten its arguments.",
    removeAsk: (name) =>
      `Remove “${name}”? New conversations stop getting it. Open ones keep it until they close.`,
    removeByNameAsk: (name) =>
      `Remove “${name}”? This removes the first server stored under that name. New conversations stop getting it. Open ones keep it until they close.`,
    removeFirst: (name) => `Remove the first server named “${name}”`,
    removeFirstAsk: (name, command) =>
      `Remove the first server named “${name}”, which runs ${command}? New conversations stop getting it. Open ones keep it until they close.`,
    nameShared: (count) =>
      `${count} servers share this name. Only the first can be removed here, and none edited.`,
    storedValueCleared: "Empty: the stored value will be cleared",
    keepStored: "Keep stored value",
    pasted: (lines, endsWithBreak) =>
      `Pasted value: ${lines === 1 ? "1 line" : `${lines} lines`}${endsWithBreak ? ", ends with a line break" : ""}`,
    trimBreak: "Remove line break",
    clearPasted: "Clear",
  },
  /** A sample session (in-memory source) that waits on an approval. */
  approvalSession: "Release build signing",
  /** Sample sessions (in-memory source) that each wait on one approval. */
  approvalSessions: ["Release build signing", "Notarize the macOS", "Reconnect storm"],
  /** The sample session whose approval carries a bidi control in its argument (#553). */
  commandOrderSession: "Command drawn in order",
  /** The sample session whose tool name is right to left, so the command's base direction is tested (#553). */
  commandBaseSession: "Command base direction",
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
  /** The card's menu for an always answer, drawn only when the review offers one. */
  moreWaysToAllow: "More Ways to Allow",
  /** The sample session (in-memory source) whose conversation carries a widget of each state. */
  widgetSession: "Widget hosts, every state",
  /** The sample session whose conversation carries the subagents card. */
  subagentsSession: "Retry budget for ACP reconnects",
  /** The channel the sample session is in. */
  widgetChannel: "design-system",
  /** The sample session (in-memory source) whose conversation carries the fixture MCP App's call. */
  appSession: "An MCP App, in its sandbox",
  /** What the fixture server refuses its hidden tool with (`fixture-plugin.ts`). */
  hiddenToolRefused: "fixture_secret is not available to apps",
  /** The fixture app's server and its tool, as a message it wrote names them (`fixture-plugin.ts`). */
  fixtureServer: "nessa-fixture",
  fixtureTool: "show_fixture",
  /** The message the fixture app sends (`fixture-app.ts`). */
  fixtureMessage: "Plot May next to April",
  /** What the sample answers a context: it has no model (`noModelForContext`). */
  noModelForContext: "The sample has no model to give context to",
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
  /** The Agents overview's control that lists every session (`overview.tsx`). */
  overviewShowAll: "Show All",
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

/**
 * The label of the option with `effect` on a conversation permission.
 * A card's button says this (#444); a surface never invents "Allow Once".
 * Null when the review offers no such option.
 */
export function offeredLabel(options, effect) {
  const option = options?.find((each) => each.effect === effect)
  return option ? option.label : null
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
  // The startup screen logs its detailed cause. The screen shows the code.
  { text: /^\[nessa\] /, url: /.*/ },
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
 * a review when the page calls a tool (`__appReview.call`), or sends a
 * message (`__appReview.message`, #390). The longest tool's name and message
 * are the page's (`__appReview.longestTool`, `.longestMessage`,
 * `.longestWord`), from the client's own bounds.
 */
export const appReview = {
  page: "verification/desktop/fixtures/app-review/index.html",
  session: "Clean up the stale rows",
  sessionId: "0b9a3c1e-5d2f-4a7b-8c6d-1e2f3a4b5c6d",
  /** The card's head: the app by its server, and the tool it named. */
  head: (tool) => names.appAsks("mcptest", tool),
  /**
   * The overview row's accessible name: the title, then the app asking, its
   * server and its command each isolated between FSI and PDI (`spoken` in
   * `said.tsx`, #390).
   */
  row: (tool) =>
    `Clean up the stale rows. ${names.appAsks("\u2068mcptest\u2069", `\u2068${tool} {}\u2069`)}.`,
  tool: "app_delete_row",
  /** The app's own tool and server, as its message's review and label name them (#390). */
  appTool: "show_rows",
  server: "mcptest",
  /** What the app sends. */
  message: "Plot May next to April",
  /** A message's review card's head (#390). */
  messageHead: names.appAsksToMessage("mcptest"),
  /** A message's overview row's accessible name. */
  messageRow: `Clean up the stale rows. ${names.appAsksToMessage("\u2068mcptest\u2069")}.`,
  /**
   * Names carrying bidi controls (E2-1 on #390) — a stray PDI then an
   * embedding, and an override — and each as the window shows it, every
   * control as U+FFFD (`shownName` in `said.tsx`).
   */
  bidiNames: [
    {
      server: "a\u2069\u202Eb",
      tool: "c\u2069\u2069\u202Bd",
      shownServer: "a\uFFFD\uFFFDb",
      shownTool: "c\uFFFD\uFFFD\uFFFDd",
    },
    {
      server: "evil\u202Egnp.exe",
      tool: "show_rows",
      shownServer: "evil\uFFFDgnp.exe",
      shownTool: "show_rows",
    },
  ],
  /** The fixture page's title, by which the script knows it is served. */
  title: "Nessa: an app's review",
}

/** Provider recovery fixture and accessible controls (#501). */
export const providerSignIn = {
  page: "verification/desktop/fixtures/provider-sign-in/index.html",
  panelPage: "verification/desktop/fixtures/provider-sign-in/panel.html",
  card: ".provider-sign-in",
  button: ".provider-sign-in button",
  failure: ".provider-sign-in [role=status]",
}

/** Controlled gateway replies under the production desktop source/store (#532). */
export const messageSync = {
  page: "verification/desktop/fixtures/message-sync/index.html",
  title: "Nessa: message synchronization",
  session: "Message synchronization",
}
