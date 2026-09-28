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
}

/** CSS selectors, grouped by the part of the window they belong to. */
export const css = {
  // The window
  surface: "[data-surface]",
  workspace: "[data-workspace]", // carries data-content (see `content`)
  anyReady: "[data-pane-key], [data-surface]",

  // Panes (data-pane-key is the pane's identity; data-pane-focused marks the focused one)
  pane: "[data-pane-key]",
  focusedPane: "[data-pane-key][data-pane-focused]",
  paneGrid: ".workspace-panes", // class: the grid the panes are laid out in — the box the drag measures
  paneHeader: ".workspace-pane-header", // class: a pane's header, which carries the drag
  paneDragHandle: "[data-drag-pane]",
  paneTitle: "[data-drag-pane] .workspace-pane-title", // class: where a pane is grabbed by its title
  composer: "[data-pane-key] textarea",
  transcript: ".workspace-transcript", // class
  dragLayer: ".workspace-drag-layer", // class: what is carried is drawn in it, below the titlebar row
  dragGhost: ".workspace-drag-ghost", // class: the translucent copy the pointer carries
  dragCarrier: ".workspace-drag-carrier", // class: holds the copy at the pointer
  dragPlaceholder: ".workspace-drag-placeholder", // class: where a drop would land
  dragShield: ".workspace-drag-shield", // class: holds the pointer while carrying
  lifted: "[data-lifted]",
  dragging: "[data-workspace][data-dragging]",
  dropAnnouncer: '[data-workspace] [role="status"][aria-live="polite"]',

  // Quick switcher (⌘K, ⌘\\)
  switcherField: '[role="dialog"] input', // the switcher's search field

  // Side columns
  sidebar: ".workspace-sidebar", // class
  sessionList: ".workspace-list", // class
  sessionRow: "[data-drag-session]",
  sidebarEdge: '[role="separator"][aria-label="Resize Sidebar"]',

  // Approval card (arranged by its own width)
  approvalCard: ".workspace-approval", // class
  approvalActions: ".workspace-approval-actions button", // class
  approvalWord: ".workspace-approval-word", // class

  // Agents overview
  overview: ".agents-overview", // class
  overviewItem: "[data-overview-item]",
  overviewRequest: ".agents-request", // class
  overviewRow: ".agents-row", // class
  overviewReplyPill: ".agents-reply-pill", // class
  overviewFilter: ".agents-filter", // class

  // Column heads (src/desktop/ui/column-header.tsx): data-title / data-placement are "inline" | "below"
  columnBar: ".desktop-column-bar", // class
  columnTitle: ".desktop-column-title[data-placement]:not(.desktop-column-sizer)", // class
  titlebarButtons: ".workspace-titlebar button, .settings-titlebar button", // class

  // Settings (data-sidebar is "open" | "closed"; --settings-sidebar-w its width)
  settings: ".settings", // class
  settingsTitlebar: ".settings-titlebar", // class

  // Elements by kind, inside a part found by one of the above
  field: "textarea",
  button: "button",
  heading: "h1, h2, h3",

  // Menus and tooltips
  menu: '[role="menu"]',
  menuItem: '[role^="menuitem"]',
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

/** localStorage keys the scripts seed before the page loads. */
export const storage = {
  layout: "nessa.desktop.workspace-layout",
  agentsOverview: "nessa.desktop.experiments.agents-overview",
  motion: "nessa.desktop.motion",
  theme: "nessa.desktop.theme",
  pictureInConversations: "nessa.desktop.picture-in-conversations",
}

/** The same-window event a stored preference announces a change on. */
export const preferenceEvents = {
  layout: "nessa:workspace-layout",
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
  home: "Home",
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
}

/**
 * How the drop announcer says a zone (`saying` in
 * src/desktop/workspace/adapters/dom/drag.ts): a vertical zone is "above" or
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
  allowOnce: "Allow Once",
  leaveSettings: "Back to nessa Agent",
  /** A sample session (in-memory source) that waits on an approval. */
  approvalSession: "Release build signing",
  /** Sample sessions (in-memory source) that each wait on one approval. */
  approvalSessions: ["Release build signing", "Notarize the macOS", "Reconnect storm"],
  denyOnce: "Deny",
  alwaysAllow: "Always Allow",
}

/** Console noise that is known to be harmless (see CHECKLIST.md, "Console errors"). */
export const harmlessConsole = [
  // A fresh browser asks for /favicon.ico, which the dev server does not serve.
  // Chrome's message does not name the URL, so both the text and the source are matched.
  { text: /status of 404/i, url: /\/favicon\.ico(\?|$)/ },
]
