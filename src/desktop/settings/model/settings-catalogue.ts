/**
 * Everything Settings holds, as data: a few categories in the sidebar, each
 * with the tabs across the top of its page, and the settings on each tab.
 * This table is the one owner of every name and description Settings shows
 * and of what its search can find; `ui/` beside it only lays them out.
 *
 * ```text
 * category (sidebar) ──▶ tab (strip under the title) ──▶ setting (a row)
 * ```
 *
 * Tab ids are unique across the whole catalogue, so a tab alone says which
 * category it belongs to (`settings-catalogue.test.ts` holds them to that).
 */
import type { WorkspaceLayoutId } from "../../model/workspace-layout"

export const settingsCategories = [
  {
    id: "general",
    label: "General",
    tabs: [
      { id: "general", label: "General", keywords: "startup login menu bar window" },
      { id: "notifications", label: "Notifications", keywords: "alerts sound banner" },
      { id: "updates", label: "Updates", keywords: "version release channel beta" },
    ],
  },
  {
    id: "appearance",
    label: "Appearance",
    tabs: [
      { id: "theme", label: "Theme", keywords: "light colour color icons" },
      { id: "header", label: "Header", keywords: "picture image scene tint greeting" },
      { id: "motion", label: "Motion", keywords: "animation reduce movement" },
    ],
  },
  {
    id: "workspace",
    label: "Workspace",
    tabs: [
      { id: "layout", label: "Layout", keywords: "columns sidebar panes split" },
      { id: "sessions", label: "Sessions", keywords: "history archive conversations" },
      { id: "keyboard", label: "Keyboard", keywords: "shortcuts keys hotkeys" },
    ],
  },
  {
    id: "models",
    label: "Models",
    tabs: [
      { id: "defaults", label: "Defaults", keywords: "model thinking fast" },
      { id: "providers", label: "Providers", keywords: "anthropic openai google api" },
    ],
  },
  {
    id: "connections",
    label: "Connections",
    tabs: [
      { id: "agents", label: "Agents", keywords: "claude code codex opencode installed" },
      { id: "accounts", label: "Accounts", keywords: "sign in github login" },
      { id: "integrations", label: "Integrations", keywords: "mcp servers tools" },
    ],
  },
  {
    id: "privacy",
    label: "Privacy & Permissions",
    tabs: [
      { id: "access", label: "Access", keywords: "permissions approval ask shield" },
      { id: "data", label: "Data", keywords: "history crash reports storage" },
    ],
  },
  {
    // The home of previews of features not settled yet; with none on offer
    // its page says so, and shows no control that would do nothing.
    id: "advanced",
    label: "Advanced",
    tabs: [
      {
        id: "experimental",
        label: "Experimental",
        keywords: "labs preview previews experiments features try",
      },
    ],
  },
  {
    id: "about",
    label: "About",
    tabs: [{ id: "about", label: "About", keywords: "version acknowledgements licence" }],
  },
] as const satisfies readonly {
  id: string
  label: string
  tabs: readonly [SettingsTabShape, ...SettingsTabShape[]]
}[]

interface SettingsTabShape {
  id: string
  label: string
  /** Words a person might search for that the tab's label does not say. */
  keywords: string
}

type Category = (typeof settingsCategories)[number]
export type SettingsCategoryId = Category["id"]
export type SettingsTabId = Category["tabs"][number]["id"]

interface SettingShape {
  id: string
  tab: SettingsTabId
  label: string
  detail?: string
  /** Words a person might search for that the label and detail do not say. */
  keywords?: string
  /**
   * Shown, but not available yet: what it changes lives outside the window
   * — the host, the gateway, an account — and is not attached. Its control is
   * disabled and says so; none may look as if it works.
   */
  pending?: true
  /**
   * The one workspace layout it applies in — what it changes is drawn only
   * there. In any other its control is disabled and its row says where it
   * applies, so it never looks as if it works where it does nothing.
   */
  layout?: WorkspaceLayoutId
}

export const settingsEntries = [
  // General › General
  {
    id: "open-at-login",
    pending: true,
    tab: "general",
    label: "Open at login",
    detail: "Nessa starts in the menu bar when you log in.",
    keywords: "startup launch",
  },
  {
    id: "menu-bar",
    pending: true,
    tab: "general",
    label: "Show in menu bar",
    detail: "The panel stays one click away while the window is closed.",
    keywords: "tray status",
  },
  {
    id: "window-opens-to",
    pending: true,
    tab: "general",
    label: "Open to",
    keywords: "home last session start",
  },
  // General › Notifications
  {
    id: "notify-needs-you",
    pending: true,
    tab: "notifications",
    label: "When a session needs you",
    detail: "An approval or a question is waiting.",
    keywords: "alert approval",
  },
  {
    id: "notify-finished",
    pending: true,
    tab: "notifications",
    label: "When a long turn finishes",
    detail: "Only for turns that ran longer than a minute.",
    keywords: "done complete",
  },
  {
    id: "notify-sound",
    pending: true,
    tab: "notifications",
    label: "Play a sound",
    keywords: "chime audio",
  },
  // General › Updates
  {
    id: "update-automatically",
    pending: true,
    tab: "updates",
    label: "Check for updates automatically",
    keywords: "auto upgrade",
  },
  {
    id: "update-channel",
    pending: true,
    tab: "updates",
    label: "Release channel",
    detail: "Beta builds arrive a week or two earlier.",
    keywords: "beta stable",
  },
  // Appearance › Theme
  { id: "theme-light", tab: "theme", label: "Light", keywords: "theme colour color" },
  { id: "icon-family", tab: "theme", label: "Icons", keywords: "symbols glyphs lucide" },
  // Appearance › Header
  {
    id: "tint-from-picture",
    tab: "header",
    label: "Tint app from header picture",
    detail: "The window borrows its colours from your picture.",
    keywords: "colour color image palette",
  },
  {
    id: "picture-in-conversations",
    tab: "header",
    label: "Show picture in conversations",
    detail: "A quiet band of it at the top of each conversation.",
    keywords: "image scene sliver pane band",
  },
  {
    id: "show-greeting",
    tab: "header",
    label: "Show greeting",
    detail: "A line of welcome above the composer.",
    keywords: "hello home",
  },
  // Appearance › Motion
  {
    id: "animations",
    tab: "motion",
    label: "Animations",
    keywords: "reduce motion transitions",
  },
  {
    id: "drifting-light",
    tab: "motion",
    label: "Drifting light",
    detail: "The light behind the window moves slowly.",
    keywords: "ambient background",
  },
  // Workspace › Layout
  {
    id: "workspace-layout",
    tab: "layout",
    label: "Layout",
    keywords: "columns sidebar classic",
  },
  {
    id: "show-session-list",
    tab: "layout",
    layout: "columns",
    label: "Show session list",
    detail: "The column of a channel's sessions, beside the sidebar.",
  },
  {
    id: "cmd-click-beside",
    tab: "layout",
    label: "⌘-click opens beside",
    detail: "Otherwise ⌘-click replaces the focused pane.",
    keywords: "command split pane",
  },
  // Workspace › Sessions
  {
    id: "keep-sessions",
    pending: true,
    tab: "sessions",
    label: "Keep finished sessions",
    keywords: "archive delete history",
  },
  {
    id: "running-first",
    tab: "sessions",
    layout: "columns",
    label: "Keep running sessions at the top",
    keywords: "sort order",
  },
  // Workspace › Keyboard
  {
    id: "shortcuts",
    tab: "keyboard",
    label: "Shortcuts",
    keywords: "keys hotkeys",
  },
  // Models › Defaults
  {
    id: "default-model",
    pending: true,
    tab: "defaults",
    label: "Model",
    keywords: "claude opus sonnet gpt",
  },
  {
    id: "default-thinking",
    pending: true,
    tab: "defaults",
    label: "Thinking",
    keywords: "reasoning effort",
  },
  {
    id: "fast-mode",
    pending: true,
    tab: "defaults",
    label: "Fast mode",
    detail: "Faster replies on models that offer it.",
    keywords: "speed",
  },
  // Models › Providers
  {
    id: "providers",
    pending: true,
    tab: "providers",
    label: "Providers",
    keywords: "anthropic openai google api key",
  },
  // Connections › Agents
  {
    id: "agents",
    pending: true,
    tab: "agents",
    label: "Agents",
    keywords: "claude code codex opencode",
  },
  // Connections › Accounts
  {
    id: "nessa-account",
    pending: true,
    tab: "accounts",
    label: "Nessa account",
    detail: "Sync settings and sessions between your Macs.",
    keywords: "sign in login",
  },
  {
    id: "github",
    pending: true,
    tab: "accounts",
    label: "GitHub",
    detail: "Lets agents open pull requests as you.",
    keywords: "git repository",
  },
  // Connections › Integrations
  {
    id: "mcp-servers",
    tab: "integrations",
    label: "MCP servers",
    keywords: "tools model context protocol",
  },
  // Privacy & Permissions › Access
  {
    id: "default-access",
    pending: true,
    tab: "access",
    label: "Access",
    keywords: "permission ask edit full",
  },
  {
    id: "remember-approvals",
    pending: true,
    tab: "access",
    label: "Remember approvals for a session",
    detail: "A command you allow once is allowed again in the same session.",
    keywords: "allow",
  },
  // Privacy & Permissions › Data
  {
    id: "crash-reports",
    pending: true,
    tab: "data",
    label: "Share crash reports",
    detail: "Only the report; never your files or conversations.",
    keywords: "diagnostics telemetry",
  },
  {
    id: "session-history",
    pending: true,
    tab: "data",
    label: "Session history",
    detail: "Kept on this Mac, in Nessa's data folder.",
    keywords: "storage finder clear",
  },
  // About
  { id: "version", tab: "about", label: "Version", keywords: "build release" },
] as const satisfies readonly SettingShape[]

type Setting = (typeof settingsEntries)[number]
export type SettingId = Setting["id"]

/** A tab as the catalogue describes it, with the category that holds it. */
export interface SettingsTab {
  id: SettingsTabId
  label: string
  category: SettingsCategoryId
}

const tabs = new Map<SettingsTabId, SettingsTab>(
  settingsCategories.flatMap((category) =>
    category.tabs.map(
      (tab) => [tab.id, { id: tab.id, label: tab.label, category: category.id }] as const,
    ),
  ),
)

const settings = new Map<SettingId, SettingShape>(
  settingsEntries.map((entry) => [entry.id, entry]),
)

/**
 * The lookups below take ids typed from this table itself, so a miss is a
 * table that contradicts its own types; they throw rather than invent an entry.
 */
function found<T>(value: T | undefined, what: string): T {
  if (value === undefined) throw new Error(`Settings has no ${what}`)
  return value
}

export function settingsCategory(id: SettingsCategoryId): Category {
  return found(
    settingsCategories.find((category) => category.id === id),
    `category ${id}`,
  )
}

export function settingsTab(id: SettingsTabId): SettingsTab {
  return found(tabs.get(id), `tab ${id}`)
}

/** The tabs of a category, in the order they sit across the top of its page. */
export function tabsOf(category: SettingsCategoryId): readonly SettingsTab[] {
  return settingsCategory(category).tabs.map((tab) => settingsTab(tab.id))
}

/**
 * Whether a category's page shows its tabs across the top: when it has more
 * than one, or when its one tab is named other than the category (Advanced ›
 * Experimental) — otherwise the tab would be a name nobody sees. Search
 * names the tab in its trail by the same rule.
 */
export function showsTabs(category: SettingsCategoryId): boolean {
  const { tabs, label } = settingsCategory(category)
  return tabs.length > 1 || tabs[0].label !== label
}

/** Where a category's page opens before a tab has been chosen. */
export function firstTabOf(category: SettingsCategoryId): SettingsTabId {
  return settingsCategory(category).tabs[0].id
}

/** A setting's name and description, as every row that shows it reads them. */
export function setting(id: SettingId): SettingShape {
  return found(settings.get(id), `setting ${id}`)
}

/** One place a search can take a person: a category, a tab, or one setting on a tab. */
export interface SettingsMatch {
  category: SettingsCategoryId
  tab: SettingsTabId
  /** Absent when the match is the tab or category itself. */
  setting?: SettingId
  label: string
  /** Where it lives, e.g. `Appearance › Theme`. */
  trail: string
}

function words(query: string): string[] {
  return query.toLowerCase().trim().split(/\s+/).filter(Boolean)
}

/**
 * How well `label` answers the query, lower is better, or `undefined` when
 * some word of the query is in neither the label nor the extra text. A label
 * that starts with the query outranks one with a word that does, which
 * outranks a match in the label anywhere, which outranks the extra text.
 */
function rank(
  terms: readonly string[],
  label: string,
  extra: string,
): number | undefined {
  const name = label.toLowerCase()
  const haystack = `${name} ${extra.toLowerCase()}`
  if (!terms.every((term) => haystack.includes(term))) return undefined
  const phrase = terms.join(" ")
  if (name.startsWith(phrase)) return 0
  if (terms.every((term) => new RegExp(`(^|[^a-z0-9])${escape(term)}`).test(name)))
    return 1
  if (terms.every((term) => name.includes(term))) return 2
  return 3
}

function escape(term: string): string {
  return term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

/**
 * Finds the categories, tabs, and settings a query names, best first and in
 * catalogue order within a rank. Categories and tabs answer to their own
 * label and keywords; a setting to its label, description, and keywords — not
 * to the name of the category it sits in, which would make every setting
 * there a result.
 */
export function searchSettings(query: string): SettingsMatch[] {
  const terms = words(query)
  if (terms.length === 0) return []
  const found: { match: SettingsMatch; rank: number; order: number }[] = []
  const add = (match: SettingsMatch, score: number | undefined) => {
    if (score !== undefined) found.push({ match, rank: score, order: found.length })
  }
  for (const category of settingsCategories) {
    const first = category.tabs[0]
    add(
      { category: category.id, tab: first.id, label: category.label, trail: "" },
      rank(terms, category.label, ""),
    )
    for (const tab of category.tabs) {
      // The first tab named like its category is the category's own result.
      if (tab === first && tab.label === category.label) continue
      add(
        { category: category.id, tab: tab.id, label: tab.label, trail: category.label },
        rank(terms, tab.label, tab.keywords),
      )
    }
  }
  for (const [id, entry] of settings) {
    const tab = settingsTab(entry.tab)
    const category = settingsCategory(tab.category)
    add(
      {
        category: category.id,
        tab: tab.id,
        setting: id,
        label: entry.label,
        trail: showsTabs(category.id)
          ? `${category.label} › ${tab.label}`
          : category.label,
      },
      rank(terms, entry.label, `${entry.detail ?? ""} ${entry.keywords ?? ""}`),
    )
  }
  return found
    .sort((a, b) => a.rank - b.rank || a.order - b.order)
    .map((entry) => entry.match)
}
