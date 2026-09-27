import {
  createContext,
  Fragment,
  memo,
  useContext,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@nessa-ui/react/context-menu"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@nessa-ui/react/dropdown-menu"
import { SplitView, SplitViewPanel, SplitViewSeparator } from "@nessa-ui/react/split-view"
import type { HostKind } from "../../../host/features"
import type { AgentId } from "../../../onboarding/model/onboarding"
import { useThemePreference } from "../../adapters/theme-preference"
import {
  agentForProvider,
  composerModels,
  type ComposerModel,
} from "../../model/composer-options"
import { Composer } from "../composer"
import { HeaderArt } from "../header-art"
import { DesktopIcon, type DesktopIconRole } from "../icons"
import { openSettings } from "../settings/settings-view"
import { ThemeMenu } from "../theme-menu"
import "./variant-two.css"

/*
 * Spike: workspace variation 2 — "spatial, two columns".
 *
 * One source list holds the organisation and its sessions: sections hold
 * channels, and a channel discloses its live sessions inline. The rest of the
 * window is a canvas of chat panes that tile side by side. Everything is
 * in-memory mock state; nothing here talks to a backend.
 */

// ─── Model ──────────────────────────────────────────────────────────────────

type Status = "running" | "approval" | "idle"
type SectionId = "starred" | "labs" | "personal"
type StepIcon = "read" | "edit" | "run" | "search"

interface Channel {
  id: string
  name: string
  section: SectionId
  private?: boolean
  topic: string
}

type Part =
  | { kind: "text"; text: string }
  | {
      kind: "step"
      icon: StepIcon
      label: string
      detail?: string
      added?: number
      removed?: number
    }
  | { kind: "code"; code: string }
  | { kind: "list"; items: string[] }

interface Message {
  id: string
  role: "user" | "agent"
  at: number
  parts: Part[]
}

interface Session {
  id: string
  channelId: string
  title: string
  agent: AgentId
  model: string
  status: Status
  at: number
  since?: number
  unread?: boolean
  activity?: string
  request?: { command: string; reason: string }
  messages: Message[]
}

interface Pane {
  id: string
  sessionId: string
}

const agents: Record<AgentId, { label: string }> = {
  claude: { label: "Claude" },
  codex: { label: "Codex" },
  opencode: { label: "OpenCode" },
}

const defaultModel: Record<AgentId, string> = {
  claude: "Claude Opus 5",
  codex: "GPT-5.6 Sol",
  opencode: "Big Pickle",
}

const sections: { id: SectionId; label: string }[] = [
  { id: "starred", label: "Starred" },
  { id: "labs", label: "Nessa Labs" },
  { id: "personal", label: "Personal" },
]

const channels: Channel[] = [
  {
    id: "desktop-app",
    name: "desktop-app",
    section: "starred",
    topic: "The Tauri desktop window: shell, home, composer",
  },
  {
    id: "release",
    name: "release",
    section: "starred",
    private: true,
    topic: "Cutting, signing and shipping builds",
  },
  {
    id: "gateway",
    name: "gateway",
    section: "labs",
    topic: "nessa-gateway and the ACP harness",
  },
  {
    id: "design-system",
    name: "design-system",
    section: "labs",
    topic: "nessa_ui components and tokens",
  },
  {
    id: "sdk",
    name: "sdk",
    section: "labs",
    topic: "nessa-sdk: models, lifecycle, public API",
  },
  {
    id: "onboarding",
    name: "onboarding",
    section: "labs",
    topic: "First run and agent detection",
  },
  {
    id: "home-lab",
    name: "home-lab",
    section: "personal",
    topic: "The NAS, the Pi, and backups",
  },
  {
    id: "writing",
    name: "writing",
    section: "personal",
    topic: "Drafts and notes",
  },
  {
    id: "dotfiles",
    name: "dotfiles",
    section: "personal",
    topic: "Shell, editor and machine setup",
  },
]

const channelById = (id: string) => channels.find((channel) => channel.id === id)

const minute = 60_000

/** A quiet two-message transcript for sessions without a written one. */
function sketch(title: string, agent: AgentId, at: number): Message[] {
  return [
    {
      id: `${title}-u`,
      role: "user",
      at: at - 6 * minute,
      parts: [
        { kind: "text", text: `Can you take a look at this: ${title.toLowerCase()}?` },
      ],
    },
    {
      id: `${title}-a`,
      role: "agent",
      at,
      parts: [
        {
          kind: "step",
          icon: "search",
          label: "Searched the workspace",
          detail: "6 results",
        },
        { kind: "step", icon: "read", label: "Read", detail: "3 files" },
        {
          kind: "text",
          text: `Done. ${agents[agent].label === "Codex" ? "The fix is small" : "It came down to one place"} — I've left the change on its own branch with a note on what I checked, so it's ready whenever you want to review it.`,
        },
      ],
    },
  ]
}

function mockSessions(now: number): Session[] {
  const ago = (minutes: number) => now - minutes * minute
  const plain = (
    id: string,
    channelId: string,
    title: string,
    agent: AgentId,
    model: string,
    minutes: number,
    extra: Partial<Session> = {},
  ): Session => ({
    id,
    channelId,
    title,
    agent,
    model,
    status: "idle",
    at: ago(minutes),
    messages: sketch(title, agent, ago(minutes)),
    ...extra,
  })

  return [
    {
      id: "split-panes",
      channelId: "desktop-app",
      title: "Spatial split panes for chat",
      agent: "claude",
      model: "Claude Opus 5",
      status: "running",
      at: ago(1),
      since: now - 38_000,
      activity: "Editing variant-two.css",
      messages: [
        {
          id: "sp-1",
          role: "user",
          at: ago(16),
          parts: [
            {
              kind: "text",
              text: "Let's try a split workspace for chat. Sessions should live under their channel in the sidebar — no third column — and I want to keep two open side by side.",
            },
          ],
        },
        {
          id: "sp-2",
          role: "agent",
          at: ago(14),
          parts: [
            { kind: "step", icon: "read", label: "Read", detail: "desktop-app.tsx" },
            { kind: "step", icon: "read", label: "Read", detail: "styles.css" },
            {
              kind: "step",
              icon: "search",
              label: "Searched for",
              detail: "SplitView · 6 results",
            },
            {
              kind: "text",
              text: "The shell already has what the canvas needs: `SplitView` owns sizing and keyboard resizing, and the sidebar's edge glow can light the seams between panes. Here's the shape I'd build:",
            },
            {
              kind: "list",
              items: [
                "Each channel discloses its live sessions inline, capped at three with **Show all**.",
                "Panes tile across the canvas with `SplitView`, each with its own composer.",
                "The focused pane is lit by `--desktop-light-edge`; the others step back slightly.",
              ],
            },
          ],
        },
        {
          id: "sp-3",
          role: "user",
          at: ago(6),
          parts: [
            {
              kind: "text",
              text: "Good. ⌘-click should open a session in a split, and so should dragging one onto the canvas.",
            },
          ],
        },
        {
          id: "sp-4",
          role: "agent",
          at: ago(2),
          parts: [
            {
              kind: "step",
              icon: "edit",
              label: "Edited",
              detail: "variant-two.tsx",
              added: 184,
              removed: 12,
            },
            {
              kind: "text",
              text: "⌘-click now opens beside the focused pane. Dragging a session shows a target on the pane under the pointer: its outer thirds split left or right, the middle replaces what's there. Styling the drop target next.",
            },
          ],
        },
      ],
    },
    plain(
      "linux-frost",
      "desktop-app",
      "Frosted sidebar on Linux",
      "opencode",
      "Big Pickle",
      34,
    ),
    plain(
      "traffic-lights",
      "desktop-app",
      "Titlebar traffic-light inset",
      "claude",
      "Claude Sonnet 5",
      130,
    ),
    plain(
      "rain",
      "desktop-app",
      "Rain density in the night scene",
      "codex",
      "GPT-5.6 Sol",
      300,
    ),
    plain(
      "peek",
      "desktop-app",
      "Edge peek hand-off flicker",
      "claude",
      "Claude Sonnet 5",
      1500,
    ),
    plain(
      "tint",
      "desktop-app",
      "Tint the window from a picture",
      "codex",
      "GPT-5.6 Sol",
      2900,
    ),
    {
      id: "notes",
      channelId: "release",
      title: "Draft 0.9.0 release notes",
      agent: "claude",
      model: "Claude Sonnet 5",
      status: "idle",
      at: ago(18),
      unread: true,
      messages: [
        {
          id: "rn-1",
          role: "user",
          at: ago(24),
          parts: [
            { kind: "text", text: "Write release notes for 0.9.0 from the merged PRs." },
          ],
        },
        {
          id: "rn-2",
          role: "agent",
          at: ago(18),
          parts: [
            {
              kind: "step",
              icon: "run",
              label: "Ran",
              detail: "gh pr list --state merged --base main",
            },
            {
              kind: "text",
              text: "Here's a draft, ordered by what people will notice first:",
            },
            {
              kind: "list",
              items: [
                "The desktop window is now Nessa's main window, beside the panel.",
                "Home opens on a rainy night scene — or a picture of your own.",
                "The composer chooses model, thinking, access and project.",
                "A long draft opens into a full page to write in.",
              ],
            },
            {
              kind: "text",
              text: "Want me to open the PR against `release/0.9`?",
            },
          ],
        },
      ],
    },
    {
      id: "notarize",
      channelId: "release",
      title: "Notarize the macOS build",
      agent: "codex",
      model: "GPT-5.6 Sol",
      status: "approval",
      at: ago(6),
      request: {
        command:
          "xcrun notarytool submit Nessa_0.9.0_aarch64.dmg --keychain-profile nessa --wait",
        reason:
          "Uploads the signed build to Apple for notarization. This uses the release keychain profile.",
      },
      messages: sketch("Notarize the macOS build", "codex", ago(7)).slice(0, 1),
    },
    {
      id: "retry",
      channelId: "gateway",
      title: "Retry budget for ACP reconnects",
      agent: "codex",
      model: "GPT-6 Astra",
      status: "approval",
      at: ago(3),
      unread: true,
      request: {
        command: "cargo test -p nessa-gateway reconnect -- --nocapture",
        reason: "Runs the reconnect tests, including the new budget-exhaustion case.",
      },
      messages: [
        {
          id: "rt-1",
          role: "user",
          at: ago(22),
          parts: [
            {
              kind: "text",
              text: "Reconnects to the ACP harness hammer the gateway after a restart. Give them a retry budget.",
            },
          ],
        },
        {
          id: "rt-2",
          role: "agent",
          at: ago(4),
          parts: [
            {
              kind: "step",
              icon: "search",
              label: "Searched for",
              detail: "reconnect · 11 results",
            },
            { kind: "step", icon: "read", label: "Read", detail: "acp/reconnect.rs" },
            {
              kind: "text",
              text: "Every client retries on a fixed 250 ms interval with no ceiling, so a restart turns into a thundering herd. I've given each client a token bucket — five attempts, one refilled every two seconds — and jittered the backoff:",
            },
            {
              kind: "code",
              code: "let delay = backoff.next().with_jitter(0.2);\nif !budget.try_take() {\n    return Err(ReconnectError::BudgetExhausted);\n}",
            },
            {
              kind: "step",
              icon: "edit",
              label: "Edited",
              detail: "acp/reconnect.rs",
              added: 58,
              removed: 14,
            },
            {
              kind: "step",
              icon: "edit",
              label: "Created",
              detail: "acp/budget.rs",
              added: 41,
            },
            {
              kind: "text",
              text: "I'd like to run the reconnect tests before calling it done.",
            },
          ],
        },
      ],
    },
    {
      ...plain(
        "backpressure",
        "gateway",
        "Backpressure on the event stream",
        "claude",
        "Claude Opus 5",
        8,
      ),
      status: "running",
      since: now - 95_000,
      activity: "Running cargo clippy",
    },
    plain(
      "token",
      "gateway",
      "Rotate the gateway token",
      "claude",
      "Claude Sonnet 5",
      190,
    ),
    plain("logs", "gateway", "Structured logs to stderr", "opencode", "MiniMax M3", 1600),
    plain(
      "chips",
      "design-system",
      "Quiet chips in the composer",
      "claude",
      "Claude Opus 5",
      45,
      {
        unread: true,
      },
    ),
    plain(
      "splitview-keys",
      "design-system",
      "SplitView keyboard resizing",
      "codex",
      "GPT-5.6 Sol",
      250,
    ),
    {
      ...plain("gpt6", "sdk", "Model catalog for GPT-6", "codex", "GPT-6 Astra", 12),
      status: "running",
      since: now - 212_000,
      activity: "Reading models.json",
    },
    plain(
      "detect",
      "onboarding",
      "Detect OpenCode on PATH",
      "opencode",
      "Big Pickle",
      2800,
    ),
    plain("nas", "home-lab", "Back up the NAS to B2", "claude", "Claude Sonnet 5", 1450),
    plain("essay", "writing", "Essay: calm software", "claude", "Claude Opus 5", 4300, {
      unread: true,
    }),
  ]
}

// ─── State ──────────────────────────────────────────────────────────────────

interface State {
  sessions: Session[]
  panes: Pane[]
  focused: string
  layout: Record<string, number>
  expanded: string[]
  collapsedSections: SectionId[]
  showAll: string[]
}

type Action =
  | { type: "open"; paneId: string; sessionId: string; create?: Session }
  | {
      type: "split"
      paneId: string
      side: "left" | "right"
      newPaneId: string
      sessionId: string
      create?: Session
    }
  | { type: "close"; paneId: string }
  | { type: "focus"; paneId: string }
  | { type: "layout"; layout: Record<string, number> }
  | { type: "equalize" }
  | { type: "channel"; channelId: string; open?: boolean }
  | { type: "section"; sectionId: SectionId }
  | { type: "showAll"; channelId: string }
  | { type: "patch"; sessionId: string; patch: Partial<Session>; append?: Message }

const MIN_PANE = 340
const MAX_PANES = 4
const CAP = 3

function equalLayout(panes: Pane[]): Record<string, number> {
  return Object.fromEntries(panes.map((pane) => [pane.id, 100 / panes.length]))
}

/** Sessions that were never written in, and are no longer on screen, go away. */
function prune(state: State): State {
  const open = new Set(state.panes.map((pane) => pane.sessionId))
  const sessions = state.sessions.filter(
    (session) => session.messages.length > 0 || open.has(session.id),
  )
  return sessions.length === state.sessions.length ? state : { ...state, sessions }
}

function reveal(state: State, sessionId: string): State {
  const session = state.sessions.find((candidate) => candidate.id === sessionId)
  if (!session) return state
  return {
    ...state,
    expanded: state.expanded.includes(session.channelId)
      ? state.expanded
      : [...state.expanded, session.channelId],
    sessions: session.unread
      ? state.sessions.map((candidate) =>
          candidate.id === sessionId ? { ...candidate, unread: false } : candidate,
        )
      : state.sessions,
  }
}

function reduce(state: State, action: Action): State {
  switch (action.type) {
    case "open": {
      const sessions = action.create ? [action.create, ...state.sessions] : state.sessions
      const panes = state.panes.map((pane) =>
        pane.id === action.paneId ? { ...pane, sessionId: action.sessionId } : pane,
      )
      return prune(
        reveal({ ...state, sessions, panes, focused: action.paneId }, action.sessionId),
      )
    }
    case "split": {
      const index = state.panes.findIndex((pane) => pane.id === action.paneId)
      if (index < 0) return state
      const sessions = action.create ? [action.create, ...state.sessions] : state.sessions
      const panes = [...state.panes]
      panes.splice(action.side === "right" ? index + 1 : index, 0, {
        id: action.newPaneId,
        sessionId: action.sessionId,
      })
      // Two panes keep their proportions, halving the one split; beyond
      // that, every pane gets an even share so none is squeezed.
      const share = state.layout[action.paneId] ?? 100 / state.panes.length
      const layout =
        panes.length > 2
          ? equalLayout(panes)
          : { ...state.layout, [action.paneId]: share / 2, [action.newPaneId]: share / 2 }
      return reveal(
        { ...state, sessions, panes, layout, focused: action.newPaneId },
        action.sessionId,
      )
    }
    case "close": {
      if (state.panes.length < 2) return state
      const index = state.panes.findIndex((pane) => pane.id === action.paneId)
      if (index < 0) return state
      const neighbour = state.panes[index - 1] ?? state.panes[index + 1]
      const panes = state.panes.filter((pane) => pane.id !== action.paneId)
      const layout = { ...state.layout }
      layout[neighbour.id] = (layout[neighbour.id] ?? 0) + (layout[action.paneId] ?? 0)
      delete layout[action.paneId]
      const focused = state.focused === action.paneId ? neighbour.id : state.focused
      return prune({ ...state, panes, layout, focused })
    }
    case "focus":
      if (state.focused === action.paneId) return state
      return { ...state, focused: action.paneId }
    case "layout":
      return { ...state, layout: action.layout }
    case "equalize":
      return { ...state, layout: equalLayout(state.panes) }
    case "channel": {
      const isOpen = state.expanded.includes(action.channelId)
      const next = action.open ?? !isOpen
      if (next === isOpen) return state
      return {
        ...state,
        expanded: next
          ? [...state.expanded, action.channelId]
          : state.expanded.filter((id) => id !== action.channelId),
      }
    }
    case "section": {
      const collapsed = state.collapsedSections.includes(action.sectionId)
      return {
        ...state,
        collapsedSections: collapsed
          ? state.collapsedSections.filter((id) => id !== action.sectionId)
          : [...state.collapsedSections, action.sectionId],
      }
    }
    case "showAll":
      return {
        ...state,
        showAll: state.showAll.includes(action.channelId)
          ? state.showAll.filter((id) => id !== action.channelId)
          : [...state.showAll, action.channelId],
      }
    case "patch":
      return {
        ...state,
        sessions: state.sessions.map((session) =>
          session.id === action.sessionId
            ? {
                ...session,
                ...action.patch,
                messages: action.append
                  ? [...session.messages, action.append]
                  : session.messages,
              }
            : session,
        ),
      }
  }
}

function initialState(): State {
  const now = Date.now()
  const wide = typeof window === "undefined" || window.innerWidth >= 1180
  const panes: Pane[] = wide
    ? [
        { id: "pane-a", sessionId: "split-panes" },
        { id: "pane-b", sessionId: "retry" },
      ]
    : [{ id: "pane-a", sessionId: "split-panes" }]
  return {
    sessions: mockSessions(now),
    panes,
    focused: "pane-a",
    layout: wide ? { "pane-a": 54, "pane-b": 46 } : equalLayout(panes),
    expanded: ["desktop-app", "gateway"],
    collapsedSections: [],
    showAll: [],
  }
}

// ─── Small helpers ──────────────────────────────────────────────────────────

const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)
const keys = (value: string) =>
  isMac ? value : value.replace("⌥", "Alt+").replace("⌘", "Ctrl+").replace("⇧", "Shift+")
const modKey = (event: { metaKey: boolean; ctrlKey: boolean }) =>
  isMac ? event.metaKey : event.ctrlKey

let sequence = 0
const makeId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${++sequence}`

function ago(at: number, now: number): string {
  const minutes = Math.max(0, Math.round((now - at) / minute))
  if (minutes < 1) return "now"
  if (minutes < 60) return `${minutes}m`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours}h`
  return `${Math.round(hours / 24)}d`
}

function byRecency(a: Session, b: Session) {
  return b.at - a.at
}

/** Inline `code` and **strong** in otherwise plain prose. */
function rich(text: string): ReactNode[] {
  return text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g).map((piece, index) => {
    if (piece.startsWith("`") && piece.endsWith("`") && piece.length > 1)
      return <code key={index}>{piece.slice(1, -1)}</code>
    if (piece.startsWith("**") && piece.endsWith("**") && piece.length > 3)
      return <strong key={index}>{piece.slice(2, -2)}</strong>
    return <Fragment key={index}>{piece}</Fragment>
  })
}

/** Subsequence match, favouring runs and word starts. */
function fuzzy(query: string, text: string): { score: number; hits: number[] } | null {
  const needle = query.toLowerCase().replace(/\s+/g, "")
  if (!needle) return { score: 0, hits: [] }
  const haystack = text.toLowerCase()
  const hits: number[] = []
  let score = 0
  let last = -2
  let at = 0
  for (let index = 0; index < haystack.length && at < needle.length; index++) {
    if (haystack[index] !== needle[at]) continue
    const wordStart = index === 0 || /[\s\-#/_.:]/.test(haystack[index - 1])
    score += 1 + (index === last + 1 ? 3 : 0) + (wordStart ? 4 : 0)
    hits.push(index)
    last = index
    at++
  }
  if (at < needle.length) return null
  return { score: score - haystack.length * 0.02, hits }
}

function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (hits.length === 0) return <>{text}</>
  const set = new Set(hits)
  return (
    <>
      {[...text].map((char, index) =>
        set.has(index) ? (
          <mark key={index} className="spike-two-hit">
            {char}
          </mark>
        ) : (
          <Fragment key={index}>{char}</Fragment>
        ),
      )}
    </>
  )
}

/**
 * One object whose methods always run the latest render's handlers. Its
 * identity never changes, so memoised children given it skip re-rendering
 * when only their parent's closures are new.
 */
function useStableActions<T extends Record<keyof T, (...args: never[]) => unknown>>(
  fresh: T,
): T {
  const latest = useRef(fresh)
  latest.current = fresh
  const [stable] = useState(() => {
    const out: Record<string, (...args: unknown[]) => unknown> = {}
    for (const key of Object.keys(fresh) as (keyof T & string)[])
      out[key] = (...args) =>
        (latest.current[key] as unknown as (...args: unknown[]) => unknown)(...args)
    return out as unknown as T
  })
  return stable
}

function useNow(interval: number) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), interval)
    return () => window.clearInterval(timer)
  }, [interval])
  return now
}

// ─── Small pieces ───────────────────────────────────────────────────────────

function StatusGlyph({ status }: { status: Status }) {
  if (status === "running")
    return (
      <span
        className="spike-two-glyph"
        data-status="running"
        role="img"
        aria-label="Running"
      />
    )
  if (status === "approval")
    return (
      <span
        className="spike-two-glyph"
        data-status="approval"
        role="img"
        aria-label="Needs approval"
      />
    )
  return <span className="spike-two-glyph" data-status="idle" aria-hidden="true" />
}

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="spike-two-kbd">{children}</kbd>
}

// ─── Root ───────────────────────────────────────────────────────────────────

type SwitcherMode = "open" | "split"
type Zone = "left" | "center" | "right"

interface SidebarActions {
  open: (sessionId: string, split: boolean) => void
  openChannel: (channelId: string, split: boolean) => void
  newSession: (channelId: string) => void
  compose: () => void
  search: () => void
  closePane: (sessionId: string) => void
  dragChange: (dragging: boolean) => void
}

interface PaneActions {
  focus: (paneId: string) => void
  close: (paneId: string) => void
  split: (paneId: string) => void
  equalize: () => void
  reveal: (sessionId: string) => void
  send: (sessionId: string, text: string) => void
  model: (sessionId: string, model: { provider: string; modelId: string }) => void
  decide: (sessionId: string, allow: boolean) => void
  drop: (paneId: string, sessionId: string, zone: Zone) => void
  canSplit: () => boolean
}
const DRAG_TYPE = "application/x-nessa-session"

/** Spike: workspace variation 2. */
export function VariantTwo({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const [theme, setTheme] = useThemePreference()
  const [state, dispatch] = useReducer(reduce, undefined, initialState)
  const [sidebarOpen, setSidebarOpen] = useState(true)
  // Which way the canvas last slid. Set only by a toggle, so the first paint
  // does not animate; a new value restarts the slide.
  const [sidebarSlide, setSidebarSlide] = useState<"open" | "close" | null>(null)
  const sidebarWas = useRef(sidebarOpen)
  useLayoutEffect(() => {
    if (sidebarWas.current === sidebarOpen) return
    sidebarWas.current = sidebarOpen
    setSidebarSlide(sidebarOpen ? "open" : "close")
  }, [sidebarOpen])
  const [switcher, setSwitcher] = useState<SwitcherMode | null>(null)
  const [dragging, setDragging] = useState(false)
  const canvasRef = useRef<HTMLDivElement>(null)
  const timers = useRef(new Set<number>())

  useEffect(() => {
    const pending = timers.current
    return () => pending.forEach((timer) => window.clearTimeout(timer))
  }, [])

  // Menus portal to the body; this marks it so they take this view's menu style.
  useEffect(() => {
    document.body.dataset.spikeTwo = ""
    return () => {
      delete document.body.dataset.spikeTwo
    }
  }, [])

  const later = useCallback((ms: number, run: () => void) => {
    const timer = window.setTimeout(() => {
      timers.current.delete(timer)
      run()
    }, ms)
    timers.current.add(timer)
  }, [])

  const focusedPane =
    state.panes.find((pane) => pane.id === state.focused) ?? state.panes[0]
  const sessionById = useCallback(
    (id: string) => state.sessions.find((session) => session.id === id),
    [state.sessions],
  )
  const focusedSession = sessionById(focusedPane.sessionId)
  const focusedChannelId = focusedSession?.channelId ?? "desktop-app"

  const roomForSplit = () =>
    state.panes.length < MAX_PANES &&
    (canvasRef.current?.clientWidth ?? 0) / (state.panes.length + 1) >= MIN_PANE

  /** Open a session: in place, or beside a pane when there is room. */
  const open = (
    sessionId: string,
    how: "replace" | "split" = "replace",
    options: { paneId?: string; side?: "left" | "right"; create?: Session } = {},
  ) => {
    const paneId = options.paneId ?? focusedPane.id
    const showing = state.panes.find((pane) => pane.sessionId === sessionId)
    if (showing && !options.create) {
      dispatch({ type: "focus", paneId: showing.id })
      dispatch({
        type: "channel",
        channelId: sessionById(sessionId)?.channelId ?? "",
        open: true,
      })
      return
    }
    if (how === "split" && roomForSplit()) {
      dispatch({
        type: "split",
        paneId,
        side: options.side ?? "right",
        newPaneId: makeId("pane"),
        sessionId,
        create: options.create,
      })
    } else {
      dispatch({ type: "open", paneId, sessionId, create: options.create })
    }
  }

  const draft = (channelId: string, agent: AgentId = "claude"): Session => ({
    id: makeId("session"),
    channelId,
    title: "New session",
    agent,
    model: defaultModel[agent],
    status: "idle",
    at: Date.now(),
    messages: [],
  })

  const newSession = (channelId: string, how: "replace" | "split" = "replace") => {
    const session = draft(channelId)
    open(session.id, how, { create: session })
    return session
  }

  const openChannel = (channelId: string, how: "replace" | "split" = "replace") => {
    const latest = state.sessions
      .filter((session) => session.channelId === channelId && session.messages.length > 0)
      .sort(byRecency)[0]
    dispatch({ type: "channel", channelId, open: true })
    if (latest) open(latest.id, how)
    else newSession(channelId, how)
  }

  /** The mock agent: thinks for a moment, then answers. */
  const respond = (session: Session, text: string) => {
    const first = session.messages.length === 0
    const now = Date.now()
    dispatch({
      type: "patch",
      sessionId: session.id,
      patch: {
        status: "running",
        activity: "Thinking",
        since: now,
        at: now,
        title: first ? text.replace(/\s+/g, " ").slice(0, 48) : session.title,
      },
      append: { id: makeId("m"), role: "user", at: now, parts: [{ kind: "text", text }] },
    })
    later(1100, () =>
      dispatch({
        type: "patch",
        sessionId: session.id,
        patch: { activity: "Reading the workspace" },
      }),
    )
    later(2600, () =>
      dispatch({
        type: "patch",
        sessionId: session.id,
        patch: { status: "idle", activity: undefined, at: Date.now() },
        append: {
          id: makeId("m"),
          role: "agent",
          at: Date.now(),
          parts: [
            {
              kind: "step",
              icon: "search",
              label: "Searched the workspace",
              detail: "4 results",
            },
            {
              kind: "text",
              text: "Got it. I'll start with the smallest change that proves the idea, and check it against the existing tests before touching anything else. I'll stop and ask before running anything outside the project.",
            },
          ],
        },
      }),
    )
  }

  const decide = (session: Session, allow: boolean) => {
    const command = session.request?.command ?? ""
    if (!allow) {
      dispatch({
        type: "patch",
        sessionId: session.id,
        patch: { status: "idle", request: undefined, at: Date.now() },
        append: {
          id: makeId("m"),
          role: "agent",
          at: Date.now(),
          parts: [
            {
              kind: "text",
              text: `Okay — I won't run it. Everything else is in place; you can run it yourself with \`${command}\`.`,
            },
          ],
        },
      })
      return
    }
    dispatch({
      type: "patch",
      sessionId: session.id,
      patch: {
        status: "running",
        request: undefined,
        since: Date.now(),
        activity: `Running ${command.split(" ").slice(0, 2).join(" ")}`,
      },
    })
    later(2400, () =>
      dispatch({
        type: "patch",
        sessionId: session.id,
        patch: { status: "idle", activity: undefined, at: Date.now() },
        append: {
          id: makeId("m"),
          role: "agent",
          at: Date.now(),
          parts: [
            { kind: "step", icon: "run", label: "Ran", detail: command },
            {
              kind: "text",
              text: "That passed cleanly. The change is ready for review whenever you are.",
            },
          ],
        },
      }),
    )
  }

  /** Closes a pane; the last one goes back to home in its channel instead. */
  const closePane = (paneId: string) => {
    if (state.panes.length > 1) {
      dispatch({ type: "close", paneId })
      return
    }
    const session = sessionById(state.panes[0].sessionId)
    if (session && session.messages.length > 0) {
      const next = draft(session.channelId)
      dispatch({ type: "open", paneId, sessionId: next.id, create: next })
    }
  }

  const revealInSidebar = (sessionId: string) => {
    const session = sessionById(sessionId)
    if (!session) return
    setSidebarOpen(true)
    dispatch({ type: "channel", channelId: session.channelId, open: true })
    requestAnimationFrame(() => {
      const row = document.querySelector<HTMLElement>(
        `.spike-two [data-session-row="${session.id}"]`,
      )
      row?.scrollIntoView({ block: "nearest", behavior: "smooth" })
      row?.focus({ preventScroll: true })
    })
  }

  const focusPaneAt = (index: number) => {
    const pane = state.panes[index]
    if (!pane) return
    dispatch({ type: "focus", paneId: pane.id })
    requestAnimationFrame(() =>
      document
        .querySelector<HTMLElement>(`#${pane.id} textarea, #${pane.id} [data-pane-focus]`)
        ?.focus(),
    )
  }

  // Window-wide keys. Read through a ref so the listener is bound once.
  const keyActions = useRef({
    switcher,
    setSwitcher,
    newSession: () => newSession(focusedChannelId),
    close: () => closePane(focusedPane.id),
    focusPaneAt,
    focusedIndex: state.panes.indexOf(focusedPane),
  })
  keyActions.current = {
    switcher,
    setSwitcher,
    newSession: () => newSession(focusedChannelId),
    close: () => closePane(focusedPane.id),
    focusPaneAt,
    focusedIndex: state.panes.indexOf(focusedPane),
  }
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!modKey(event)) return
      const actions = keyActions.current
      const key = event.key.toLowerCase()
      if (key === "k" && !event.altKey) {
        event.preventDefault()
        actions.setSwitcher(actions.switcher ? null : "open")
      } else if (actions.switcher) {
        return
      } else if (event.key === "\\") {
        event.preventDefault()
        actions.setSwitcher("split")
      } else if (key === "n" && !event.shiftKey) {
        event.preventDefault()
        actions.newSession()
      } else if (key === "w") {
        event.preventDefault()
        actions.close()
      } else if (key === "b" && !event.altKey) {
        event.preventDefault()
        setSidebarOpen((value) => !value)
      } else if (/^[1-4]$/.test(event.key)) {
        event.preventDefault()
        actions.focusPaneAt(Number(event.key) - 1)
      } else if (
        event.altKey &&
        (event.key === "ArrowLeft" || event.key === "ArrowRight")
      ) {
        event.preventDefault()
        actions.focusPaneAt(actions.focusedIndex + (event.key === "ArrowLeft" ? -1 : 1))
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])

  const paneOf = useMemo(() => {
    const map = new Map<string, number>()
    state.panes.forEach((pane, index) => map.set(pane.sessionId, index + 1))
    return map
  }, [state.panes])

  const sidebarActions = useStableActions<SidebarActions>({
    open: (id, split) => open(id, split ? "split" : "replace"),
    openChannel: (id, split) => openChannel(id, split ? "split" : "replace"),
    newSession: (channelId) => {
      newSession(channelId)
    },
    compose: () => {
      newSession(focusedChannelId)
    },
    search: () => setSwitcher("open"),
    closePane: (sessionId) => {
      const pane = state.panes.find((candidate) => candidate.sessionId === sessionId)
      if (pane) closePane(pane.id)
    },
    dragChange: setDragging,
  })

  const paneActions = useStableActions<PaneActions>({
    focus: (paneId) => dispatch({ type: "focus", paneId }),
    close: closePane,
    split: (paneId) => {
      dispatch({ type: "focus", paneId })
      setSwitcher("split")
    },
    equalize: () => dispatch({ type: "equalize" }),
    reveal: revealInSidebar,
    send: (sessionId, text) => {
      const session = sessionById(sessionId)
      if (session) respond(session, text)
    },
    model: (sessionId, { provider, modelId }) => {
      const next = composerModels.find(
        (candidate) => candidate.provider === provider && candidate.modelId === modelId,
      )
      const session = sessionById(sessionId)
      if (!next || !session) return
      dispatch({
        type: "patch",
        sessionId,
        patch: {
          model: next.displayName,
          agent: agentForProvider(provider) ?? session.agent,
        },
      })
    },
    decide: (sessionId, allow) => {
      const session = sessionById(sessionId)
      if (session) decide(session, allow)
    },
    drop: (paneId, sessionId, zone) => {
      setDragging(false)
      if (zone === "center") open(sessionId, "replace", { paneId })
      else open(sessionId, "split", { paneId, side: zone })
    },
    canSplit: roomForSplit,
  })

  const multi = state.panes.length > 1
  const sidebarLabel = `${sidebarOpen ? "Hide" : "Show"} Sidebar (${keys("⌘B")})`
  const composeLabel = `New Session (${keys("⌘N")})`

  return (
    <div
      className="spike-two"
      data-host={hostKind}
      data-surface={browserSurface ? "browser" : "window"}
      data-desktop-theme={theme}
      data-sidebar={sidebarOpen ? "open" : "closed"}
      data-sidebar-slide={sidebarSlide ?? undefined}
      data-dragging={dragging || undefined}
    >
      <div className="desktop-ambient" aria-hidden="true">
        <span className="desktop-grain" />
      </div>

      {/* The window's own row: native buttons, then the sidebar toggle, fixed
          in place whether the sidebar is open or not. */}
      <div className="spike-two-titlebar" data-tauri-drag-region>
        <button
          type="button"
          className="spike-two-icon-button"
          aria-label={sidebarLabel}
          title={sidebarLabel}
          aria-expanded={sidebarOpen}
          aria-controls="spike-two-sidebar"
          onClick={() => setSidebarOpen((value) => !value)}
        >
          <DesktopIcon name="sidebar" />
        </button>
        <button
          type="button"
          className="spike-two-icon-button spike-two-titlebar-compose"
          aria-label={composeLabel}
          title={composeLabel}
          tabIndex={sidebarOpen ? -1 : 0}
          aria-hidden={sidebarOpen || undefined}
          onClick={() => newSession(focusedChannelId)}
        >
          <DesktopIcon name="newSession" />
        </button>
      </div>

      <MultiPaneContext.Provider value={multi}>
        <SourceList
          open={sidebarOpen}
          state={state}
          dispatch={dispatch}
          paneOf={paneOf}
          focusedSessionId={focusedPane.sessionId}
          theme={theme}
          onThemeChange={setTheme}
          actions={sidebarActions}
        />
      </MultiPaneContext.Provider>

      <main className="spike-two-canvas" ref={canvasRef} aria-label="Conversations">
        <SplitView
          className="spike-two-split"
          layout={state.layout}
          onLayoutChange={(layout) => dispatch({ type: "layout", layout })}
        >
          {state.panes.map((pane, index) => {
            const session = sessionById(pane.sessionId)
            if (!session) return null
            return (
              <Fragment key={pane.id}>
                {index > 0 ? (
                  <SplitViewSeparator
                    className="spike-two-seam"
                    aria-label="Resize panes"
                    onPointerEnter={positionGlow}
                    onPointerMove={positionGlow}
                    onDoubleClick={() => dispatch({ type: "equalize" })}
                    title="Drag to resize · double-click to even out"
                  />
                ) : null}
                <SplitViewPanel id={pane.id} minSize={`${MIN_PANE - 20}px`}>
                  <ChatPane
                    paneId={pane.id}
                    first={index === 0}
                    session={session}
                    focused={pane.id === focusedPane.id}
                    multi={multi}
                    dragging={dragging}
                    actions={paneActions}
                  />
                </SplitViewPanel>
              </Fragment>
            )
          })}
        </SplitView>
      </main>

      {switcher ? (
        <QuickSwitcher
          mode={switcher}
          sessions={state.sessions}
          focusedChannelId={focusedChannelId}
          onClose={() => setSwitcher(null)}
          onPick={(pick, split) => {
            const how = split || switcher === "split" ? "split" : "replace"
            setSwitcher(null)
            if (pick.kind === "session") open(pick.id, how)
            else if (pick.kind === "channel") openChannel(pick.id, how)
            else {
              const session = newSession(pick.channelId, how)
              if (pick.text) respond(session, pick.text)
            }
          }}
        />
      ) : null}
    </div>
  )
}

/** Lights the seam where the pointer is, like the sidebar edges. */
function positionGlow(event: ReactPointerEvent<HTMLDivElement>) {
  const edge = event.currentTarget
  edge.style.setProperty(
    "--edge-glow-y",
    `${event.clientY - edge.getBoundingClientRect().top}px`,
  )
}

// ─── Source list ────────────────────────────────────────────────────────────

const SourceList = memo(function SourceList({
  open,
  state,
  dispatch,
  paneOf,
  focusedSessionId,
  theme,
  onThemeChange,
  actions,
}: {
  open: boolean
  state: State
  dispatch: (action: Action) => void
  paneOf: Map<string, number>
  focusedSessionId: string
  theme: Parameters<typeof ThemeMenu>[0]["theme"]
  onThemeChange: Parameters<typeof ThemeMenu>[0]["onThemeChange"]
  actions: SidebarActions
}) {
  const now = useNow(30_000)
  // Each channel's sessions, newest first. A draft is not a session until it
  // is sent, so it never appears here.
  const byChannel = useMemo(() => {
    const map = new Map<string, Session[]>()
    for (const session of [...state.sessions].sort(byRecency)) {
      if (session.messages.length === 0) continue
      const list = map.get(session.channelId)
      if (list) list.push(session)
      else map.set(session.channelId, [session])
    }
    return map
  }, [state.sessions])
  const treeRef = useRef<HTMLElement>(null)
  const waiting = state.sessions.filter((session) => session.status === "approval")
  const focusedChannel = state.sessions.find(
    (session) => session.id === focusedSessionId,
  )?.channelId

  // Arrow keys walk the visible rows; left and right fold and unfold.
  const onTreeKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const tree = treeRef.current
    if (!tree) return
    const rows = [...tree.querySelectorAll<HTMLElement>("[data-row]")].filter(
      (row) => !row.closest("[inert]"),
    )
    const current = document.activeElement as HTMLElement | null
    const index = current ? rows.indexOf(current) : -1
    if (index < 0) return
    const row = rows[index]
    const move = (to: number) => {
      event.preventDefault()
      rows[Math.max(0, Math.min(rows.length - 1, to))]?.focus()
    }
    if (event.key === "ArrowDown") move(index + 1)
    else if (event.key === "ArrowUp") move(index - 1)
    else if (event.key === "Home") move(0)
    else if (event.key === "End") move(rows.length - 1)
    else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
      const opening = event.key === "ArrowRight"
      const expanded = row.getAttribute("aria-expanded") === "true"
      if (row.dataset.row === "channel" && row.dataset.channel) {
        event.preventDefault()
        if (opening === expanded) move(index + (opening ? 1 : 0))
        else dispatch({ type: "channel", channelId: row.dataset.channel, open: opening })
      } else if (row.dataset.row === "section" && row.dataset.section) {
        event.preventDefault()
        if (opening !== expanded)
          dispatch({ type: "section", sectionId: row.dataset.section as SectionId })
      } else if (!opening && row.dataset.parent) {
        event.preventDefault()
        tree.querySelector<HTMLElement>(`[data-channel="${row.dataset.parent}"]`)?.focus()
      }
    } else if (event.key === "Enter" && modKey(event)) {
      event.preventDefault()
      if (row.dataset.sessionRow) actions.open(row.dataset.sessionRow, true)
      else if (row.dataset.channel) actions.openChannel(row.dataset.channel, true)
    }
  }

  return (
    <aside
      id="spike-two-sidebar"
      className="spike-two-sidebar"
      data-open={open || undefined}
      inert={!open}
      aria-hidden={!open || undefined}
      aria-label="Workspace"
    >
      <div className="spike-two-glass">
        <div className="spike-two-sidebar-top" data-tauri-drag-region>
          <button
            type="button"
            className="spike-two-icon-button"
            aria-label={`New Session (${keys("⌘N")})`}
            title={`New Session (${keys("⌘N")})`}
            onClick={actions.compose}
          >
            <DesktopIcon name="newSession" />
          </button>
        </div>

        <div className="spike-two-sidebar-scroll">
          <button type="button" className="spike-two-search" onClick={actions.search}>
            <DesktopIcon name="search" />
            <span>Search</span>
            <Kbd>{keys("⌘K")}</Kbd>
          </button>

          {waiting.length > 0 ? (
            <button
              type="button"
              className="spike-two-needs"
              onClick={() => actions.open(waiting[0].id, false)}
              title="Open the next session waiting on you"
            >
              <StatusGlyph status="approval" />
              <span>Needs you</span>
              <span className="spike-two-count">{waiting.length}</span>
            </button>
          ) : null}

          <nav
            ref={treeRef}
            className="spike-two-tree"
            aria-label="Channels"
            onKeyDown={onTreeKeyDown}
          >
            {sections.map((section) => {
              const collapsed = state.collapsedSections.includes(section.id)
              const inSection = channels.filter(
                (channel) => channel.section === section.id,
              )
              return (
                <div key={section.id} className="spike-two-section">
                  <button
                    type="button"
                    className="spike-two-section-label"
                    data-row="section"
                    data-section={section.id}
                    aria-expanded={!collapsed}
                    onClick={() => dispatch({ type: "section", sectionId: section.id })}
                  >
                    <span>{section.label}</span>
                    <DesktopIcon
                      name="chevronRight"
                      className="spike-two-section-chevron"
                    />
                  </button>
                  <div className="spike-two-fold" data-open={!collapsed || undefined}>
                    <ul inert={collapsed} className="spike-two-fold-inner">
                      {inSection.map((channel) => (
                        <ChannelBranch
                          key={channel.id}
                          channel={channel}
                          sessions={byChannel.get(channel.id) ?? noSessions}
                          expanded={state.expanded.includes(channel.id)}
                          showAll={state.showAll.includes(channel.id)}
                          current={channel.id === focusedChannel}
                          focusedSessionId={
                            channel.id === focusedChannel ? focusedSessionId : null
                          }
                          openIds={(byChannel.get(channel.id) ?? noSessions)
                            .filter((session) => paneOf.has(session.id))
                            .map((session) => session.id)
                            .join(",")}
                          now={now}
                          dispatch={dispatch}
                          actions={actions}
                        />
                      ))}
                    </ul>
                  </div>
                </div>
              )
            })}
          </nav>
        </div>

        <footer className="spike-two-identity">
          <span aria-hidden="true" className="desktop-mark" />
          <button
            type="button"
            className="spike-two-identity-name spike-identity-button"
            onClick={openSettings}
            title="Settings (⌘,)"
          >
            <strong>nessa</strong>
            <span>Studio</span>
          </button>
          <ThemeMenu theme={theme} onThemeChange={onThemeChange} />
        </footer>
      </div>
    </aside>
  )
})

const noSessions: Session[] = []

const ChannelBranch = memo(function ChannelBranch({
  channel,
  sessions,
  expanded,
  showAll,
  current,
  focusedSessionId,
  openIds,
  now,
  dispatch,
  actions,
}: {
  channel: Channel
  sessions: Session[]
  expanded: boolean
  showAll: boolean
  current: boolean
  /** The focused pane's session, when it is in this channel. */
  focusedSessionId: string | null
  /** The ids of this channel's sessions on screen in a pane, comma-joined. */
  openIds: string
  now: number
  dispatch: (action: Action) => void
  actions: SidebarActions
}) {
  const openSet = new Set(openIds.split(","))
  const visible = showAll
    ? sessions
    : sessions.filter(
        (session, index) =>
          index < CAP || openSet.has(session.id) || session.status === "approval",
      )
  const hidden = sessions.length - visible.length
  const waiting = sessions.some((session) => session.status === "approval")
  const running = sessions.some((session) => session.status === "running")
  const unread = sessions.some((session) => session.unread)
  const channelIcon = channel.private ? "privateChannel" : "channel"

  return (
    <li className="spike-two-branch" data-expanded={expanded || undefined}>
      <div className="spike-two-row-wrap">
        <button
          type="button"
          className="spike-two-channel"
          data-row="channel"
          data-channel={channel.id}
          data-current={current || undefined}
          data-unread={unread || undefined}
          aria-expanded={expanded}
          title={channel.topic}
          onClick={(event) => actions.openChannel(channel.id, modKey(event))}
        >
          <span
            className="spike-two-disclosure"
            aria-hidden="true"
            onClick={(event) => {
              event.stopPropagation()
              dispatch({ type: "channel", channelId: channel.id })
            }}
          >
            <DesktopIcon name={channelIcon} className="spike-two-disclosure-icon" />
            <DesktopIcon name="chevronRight" className="spike-two-disclosure-chevron" />
          </span>
          <span className="spike-two-channel-name">{channel.name}</span>
          {!expanded && (waiting || running) ? (
            <span className="spike-two-summary">
              <StatusGlyph status={waiting ? "approval" : "running"} />
            </span>
          ) : null}
        </button>
        <button
          type="button"
          className="spike-two-row-action"
          aria-label={`New session in #${channel.name}`}
          title={`New session in #${channel.name}`}
          tabIndex={-1}
          onClick={() => actions.newSession(channel.id)}
        >
          <DesktopIcon name="add" />
        </button>
      </div>

      <div className="spike-two-fold" data-open={expanded || undefined}>
        <ul className="spike-two-fold-inner spike-two-sessions" inert={!expanded}>
          {visible.map((session) => (
            <SessionRow
              key={session.id}
              session={session}
              channelId={channel.id}
              focused={session.id === focusedSessionId}
              open={openSet.has(session.id)}
              now={now}
              actions={actions}
            />
          ))}
          {hidden > 0 || (showAll && sessions.length > CAP) ? (
            <li>
              <button
                type="button"
                className="spike-two-more"
                data-row="more"
                data-parent={channel.id}
                onClick={() => dispatch({ type: "showAll", channelId: channel.id })}
              >
                {showAll ? "Show fewer" : `Show all ${sessions.length}`}
              </button>
            </li>
          ) : null}
          {sessions.length === 0 ? (
            <li>
              <button
                type="button"
                className="spike-two-more"
                data-row="more"
                data-parent={channel.id}
                onClick={() => actions.newSession(channel.id)}
              >
                Start a session
              </button>
            </li>
          ) : null}
        </ul>
      </div>
    </li>
  )
})

/** Whether more than one pane is open; read only by menus when they open. */
const MultiPaneContext = createContext(false)

const SessionRow = memo(function SessionRow({
  session,
  channelId,
  focused,
  open,
  now,
  actions,
}: {
  session: Session
  channelId: string
  focused: boolean
  open: boolean
  now: number
  actions: SidebarActions
}) {
  return (
    <li>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <button
            type="button"
            className="spike-two-session"
            data-row="session"
            data-session-row={session.id}
            data-parent={channelId}
            data-focused={focused || undefined}
            data-open={open || undefined}
            data-unread={session.unread || undefined}
            data-status={session.status}
            aria-current={focused ? "page" : undefined}
            draggable
            onDragStart={(event: DragEvent<HTMLButtonElement>) => {
              event.dataTransfer.setData(DRAG_TYPE, session.id)
              event.dataTransfer.setData("text/plain", session.title)
              event.dataTransfer.effectAllowed = "copyMove"
              actions.dragChange(true)
            }}
            onDragEnd={() => actions.dragChange(false)}
            onClick={(event: ReactMouseEvent) => actions.open(session.id, modKey(event))}
            title={`${session.title} — ${keys("⌘")}-click to open beside`}
          >
            <StatusGlyph status={session.status} />
            <span className="spike-two-session-title">{session.title}</span>
            <span className="spike-two-time">{ago(session.at, now)}</span>
          </button>
        </ContextMenuTrigger>
        <ContextMenuContent className="desktop-popover spike-two-menu">
          <ContextMenuItem onSelect={() => actions.open(session.id, false)}>
            Open
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => actions.open(session.id, true)}>
            Open Beside
            <ContextMenuShortcut>{keys("⌘")}Click</ContextMenuShortcut>
          </ContextMenuItem>
          {open ? <ClosePaneItem sessionId={session.id} actions={actions} /> : null}
        </ContextMenuContent>
      </ContextMenu>
    </li>
  )
})

function ClosePaneItem({
  sessionId,
  actions,
}: {
  sessionId: string
  actions: SidebarActions
}) {
  if (!useContext(MultiPaneContext)) return null
  return (
    <>
      <ContextMenuSeparator />
      <ContextMenuItem onSelect={() => actions.closePane(sessionId)}>
        Close Pane
      </ContextMenuItem>
    </>
  )
}

// ─── Chat pane ──────────────────────────────────────────────────────────────

/** The catalog entry a session runs on, found by the name it is shown with. */
function catalogModel(name: string): ComposerModel | undefined {
  return composerModels.find((model) => model.displayName === name)
}

// The docked composer is always a card; only home opens into a page.
const stayCard = () => {}

const ChatPane = memo(function ChatPane({
  paneId,
  first,
  session,
  focused,
  multi,
  dragging,
  actions,
}: {
  paneId: string
  first: boolean
  session: Session
  focused: boolean
  multi: boolean
  dragging: boolean
  actions: PaneActions
}) {
  const channel = channelById(session.channelId)
  const onFocus = () => actions.focus(paneId)
  const onClose = () => actions.close(paneId)
  const onSplit = () => actions.split(paneId)
  const onReveal = () => actions.reveal(session.id)
  const empty = session.messages.length === 0

  // A new pane answers on the frame it was asked for: its shell starts to
  // fade in at once and its conversation fills in on the next frame, under
  // the fade, so a split never waits on the heavier content.
  const [filled, setFilled] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setFilled(true))
    return () => cancelAnimationFrame(frame)
  }, [])
  // The header names the session only once its heading has scrolled away;
  // a new session, showing home, has no name to give yet.
  const [headingShown, setHeadingShown] = useState(true)
  const titled = !empty && !headingShown

  const closeLabel = `Close Pane (${keys("⌘W")})`
  // The last pane closes back to home; home itself has nothing to close.
  const closable = multi || !empty
  return (
    <section
      className="spike-two-pane"
      data-focused={focused || undefined}
      data-multi={multi || undefined}
      data-first={first || undefined}
      data-empty={empty || undefined}
      aria-label={`${session.title} in #${channel?.name ?? ""}`}
      onPointerDownCapture={onFocus}
      onFocusCapture={onFocus}
    >
      <header className="spike-two-pane-head" data-tauri-drag-region>
        <div
          className="spike-two-pane-title"
          data-shown={titled || undefined}
          aria-hidden={!titled || undefined}
          data-tauri-drag-region
        >
          <span className="spike-two-pane-name">{empty ? "" : session.title}</span>
        </div>
        <div className="spike-two-pane-tools">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="spike-two-icon-button"
                aria-label="Pane Actions"
                title="Pane Actions"
              >
                <DesktopIcon name="moreHorizontal" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              sideOffset={6}
              className="desktop-popover spike-two-menu"
            >
              <DropdownMenuItem onSelect={onSplit}>
                Open Beside…
                <DropdownMenuShortcut>{keys("⌘\\")}</DropdownMenuShortcut>
              </DropdownMenuItem>
              {multi ? (
                <DropdownMenuItem onSelect={actions.equalize}>
                  Even Out Panes
                </DropdownMenuItem>
              ) : null}
              {empty ? null : (
                <DropdownMenuItem onSelect={onReveal}>Show in Sidebar</DropdownMenuItem>
              )}
              {closable ? (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={onClose}>
                    Close Pane
                    <DropdownMenuShortcut>{keys("⌘W")}</DropdownMenuShortcut>
                  </DropdownMenuItem>
                </>
              ) : null}
            </DropdownMenuContent>
          </DropdownMenu>
          {/* Kept in place when there is nothing to close, so the menu never moves. */}
          <button
            type="button"
            className="spike-two-icon-button"
            aria-label={closeLabel}
            title={closeLabel}
            data-reserved={!closable || undefined}
            tabIndex={closable ? 0 : -1}
            aria-hidden={!closable || undefined}
            onClick={closable ? onClose : undefined}
          >
            <DesktopIcon name="close" />
          </button>
        </div>
      </header>

      <div className="spike-two-pane-body">
        {!filled ? null : empty ? (
          <div className="spike-two-home">
            <PaneHome sessionId={session.id} actions={actions} />
          </div>
        ) : (
          <>
            <Transcript
              session={session}
              channel={channel}
              actions={actions}
              onHeadingShown={setHeadingShown}
            />
            <div className="spike-two-dock">
              <DockComposer
                key={session.id}
                sessionId={session.id}
                provider={catalogModel(session.model)?.provider}
                modelId={catalogModel(session.model)?.modelId}
                actions={actions}
              />
            </div>
          </>
        )}
      </div>

      {dragging ? (
        <DropTarget
          allowSplit={actions.canSplit()}
          onDrop={(sessionId, zone) => actions.drop(paneId, sessionId, zone)}
        />
      ) : null}
    </section>
  )
})

function DropTarget({
  allowSplit,
  onDrop,
}: {
  allowSplit: boolean
  onDrop: (sessionId: string, zone: Zone) => void
}) {
  const [zone, setZone] = useState<Zone | null>(null)
  const zoneFor = (event: DragEvent<HTMLDivElement>): Zone => {
    if (!allowSplit) return "center"
    const box = event.currentTarget.getBoundingClientRect()
    const x = (event.clientX - box.left) / box.width
    return x < 0.3 ? "left" : x > 0.7 ? "right" : "center"
  }
  const labels: Record<Zone, string> = {
    left: "Open on Left",
    center: "Open Here",
    right: "Open on Right",
  }
  return (
    <div
      className="spike-two-drop"
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes(DRAG_TYPE)) return
        event.preventDefault()
        event.dataTransfer.dropEffect = "move"
        const next = zoneFor(event)
        if (next !== zone) setZone(next)
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null))
          setZone(null)
      }}
      onDrop={(event) => {
        event.preventDefault()
        const id = event.dataTransfer.getData(DRAG_TYPE)
        const at = zone ?? zoneFor(event)
        setZone(null)
        if (id) onDrop(id, at)
      }}
    >
      {/* Always mounted, so moving between zones glides rather than re-popping. */}
      <div className="spike-two-drop-hint" data-zone={zone ?? undefined}>
        <span className="spike-two-drop-label">
          {zone ? labels[zone] : labels.center}
        </span>
      </div>
    </div>
  )
}

/**
 * A new session's home: the same header art, greeting and composer as the
 * app's `Home`, laid out by its stylesheet, but with the composer wired to
 * this session, so a model chosen here is the one the session keeps.
 * Memoised, like the docked composer, so a pane re-rendered for its header or
 * focus leaves it alone.
 */
const PaneHome = memo(function PaneHome({
  sessionId,
  actions,
}: {
  sessionId: string
  actions: PaneActions
}) {
  const [page, setPage] = useState(false)
  return (
    <div className="desktop-home" data-page={page || undefined}>
      <div className="desktop-home-stack">
        <HeaderArt />
        <div className="desktop-home-inner">
          <h1 className="desktop-greeting">Working late?</h1>
          <Composer
            page={page}
            onPageChange={setPage}
            onSend={(text) => actions.send(sessionId, text)}
            onModelChange={(model) => actions.model(sessionId, model)}
          />
        </div>
      </div>
    </div>
  )
})

/** The docked composer, starting on the session's own model. */
const DockComposer = memo(function DockComposer({
  sessionId,
  provider,
  modelId,
  actions,
}: {
  sessionId: string
  provider: string | undefined
  modelId: string | undefined
  actions: PaneActions
}) {
  return (
    <Composer
      page={false}
      onPageChange={stayCard}
      initialModel={provider && modelId ? { provider, modelId } : undefined}
      placeholder="Reply…"
      onSend={(text) => actions.send(sessionId, text)}
      onModelChange={(model) => actions.model(sessionId, model)}
    />
  )
})

const Transcript = memo(function Transcript({
  session,
  channel,
  actions,
  onHeadingShown,
}: {
  session: Session
  channel: Channel | undefined
  actions: PaneActions
  onHeadingShown: (shown: boolean) => void
}) {
  const onReveal = () => actions.reveal(session.id)
  const onDecide = (allow: boolean) => actions.decide(session.id, allow)
  const scrollRef = useRef<HTMLDivElement>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const seen = useRef<string | null>(null)
  const pinned = useRef(true)
  const now = useNow(60_000)

  // Arriving at a session lands on its latest turn; new turns glide into view.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    const arriving = seen.current !== session.id
    seen.current = session.id
    pinned.current = true
    scroller.scrollTo({
      top: scroller.scrollHeight,
      behavior: arriving ? "auto" : "smooth",
    })
  }, [session.id, session.messages.length, session.status])

  // While the reader is at the latest turn, a pane resized by a split, the
  // sidebar or a window resize keeps them there instead of drifting upward.
  useEffect(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    const onScroll = () => {
      pinned.current =
        scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24
    }
    const observer = new ResizeObserver(() => {
      if (pinned.current) scroller.scrollTop = scroller.scrollHeight
    })
    observer.observe(scroller)
    if (scroller.firstElementChild) observer.observe(scroller.firstElementChild)
    scroller.addEventListener("scroll", onScroll, { passive: true })
    return () => {
      observer.disconnect()
      scroller.removeEventListener("scroll", onScroll)
    }
  }, [])

  // The heading hands the title to the pane header once it scrolls out.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const heading = headingRef.current
    if (!scroller || !heading) return
    const box = heading.getBoundingClientRect()
    onHeadingShown(box.bottom > scroller.getBoundingClientRect().top + 8)
    const observer = new IntersectionObserver(
      ([entry]) => onHeadingShown(entry.isIntersecting),
      { root: scroller, rootMargin: "-8px 0px 0px 0px" },
    )
    observer.observe(heading)
    return () => observer.disconnect()
  }, [session.id, onHeadingShown])

  const started = session.messages[0]?.at ?? session.at
  const startedAgo = ago(started, now)

  return (
    <div className="spike-two-scroll" ref={scrollRef} tabIndex={-1} data-pane-focus>
      <div className="spike-two-transcript">
        <div className="spike-two-intro">
          <h2 ref={headingRef}>{session.title}</h2>
          <p>
            <button type="button" className="spike-two-intro-channel" onClick={onReveal}>
              #{channel?.name}
            </button>
            <span aria-hidden="true"> · </span>
            started {startedAgo === "now" ? "just now" : `${startedAgo} ago`}
          </p>
        </div>

        {session.messages.map((message) =>
          message.role === "user" ? (
            <div key={message.id} className="spike-two-message" data-role="user">
              <div className="spike-two-bubble">
                {message.parts.map((part, index) =>
                  part.kind === "text" ? <p key={index}>{rich(part.text)}</p> : null,
                )}
              </div>
            </div>
          ) : (
            <div key={message.id} className="spike-two-message" data-role="agent">
              <AgentParts parts={message.parts} />
            </div>
          ),
        )}

        {session.status === "running" ? (
          <LiveRow
            activity={session.activity ?? "Working"}
            since={session.since ?? session.at}
          />
        ) : null}

        {session.status === "approval" && session.request ? (
          <div className="spike-two-approval" role="group" aria-label="Approval needed">
            <div className="spike-two-approval-head">
              <DesktopIcon name="needsYou" />
              <span>{agents[session.agent].label} wants to run a command</span>
            </div>
            <pre className="spike-two-approval-command">
              <span aria-hidden="true">$ </span>
              {session.request.command}
            </pre>
            <p className="spike-two-approval-reason">{session.request.reason}</p>
            <div className="spike-two-approval-actions">
              <button
                type="button"
                className="spike-two-button"
                onClick={() => onDecide(false)}
              >
                Deny
              </button>
              <div className="spike-two-approval-allow">
                <button
                  type="button"
                  className="spike-two-button"
                  onClick={() => onDecide(true)}
                >
                  Always Allow
                </button>
                <button
                  type="button"
                  className="spike-two-button"
                  data-primary
                  onClick={() => onDecide(true)}
                >
                  Allow Once
                </button>
              </div>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  )
})

function AgentParts({ parts }: { parts: Part[] }) {
  // Consecutive steps read as one quiet group.
  const groups: (Part | Part[])[] = []
  for (const part of parts) {
    const last = groups[groups.length - 1]
    if (part.kind === "step" && Array.isArray(last)) last.push(part)
    else groups.push(part.kind === "step" ? [part] : part)
  }
  const icons: Record<StepIcon, DesktopIconRole> = {
    read: "file",
    edit: "edit",
    run: "terminal",
    search: "search",
  }
  return (
    <div className="spike-two-message-body">
      {groups.map((group, index) => {
        if (Array.isArray(group))
          return (
            <ul key={index} className="spike-two-steps">
              {group.map((step, stepIndex) => {
                if (step.kind !== "step") return null
                return (
                  <li key={stepIndex}>
                    <DesktopIcon name={icons[step.icon]} />
                    <span className="spike-two-step-label">{step.label}</span>
                    {step.detail ? (
                      <span className="spike-two-step-detail">{step.detail}</span>
                    ) : null}
                    {step.added !== undefined ? (
                      <span className="spike-two-diff">
                        <ins>+{step.added}</ins>
                        {step.removed ? <del>−{step.removed}</del> : null}
                      </span>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          )
        if (group.kind === "text") return <p key={index}>{rich(group.text)}</p>
        if (group.kind === "code")
          return (
            <pre key={index} className="spike-two-code">
              <code>{group.code}</code>
            </pre>
          )
        if (group.kind === "list")
          return (
            <ul key={index} className="spike-two-list">
              {group.items.map((item) => (
                <li key={item}>{rich(item)}</li>
              ))}
            </ul>
          )
        return null
      })}
    </div>
  )
}

function LiveRow({ activity, since }: { activity: string; since: number }) {
  const now = useNow(1000)
  const seconds = Math.max(0, Math.round((now - since) / 1000))
  const elapsed =
    seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
  return (
    <div className="spike-two-live" role="status">
      <StatusGlyph status="running" />
      <span className="spike-two-shimmer">{activity}</span>
      <span className="spike-two-live-time">{elapsed}</span>
    </div>
  )
}

// ─── Quick switcher ─────────────────────────────────────────────────────────

type Pick =
  | { kind: "session"; id: string }
  | { kind: "channel"; id: string }
  | { kind: "new"; channelId: string; text?: string }

interface Row {
  key: string
  pick: Pick
  group: string
  render: ReactNode
}

function QuickSwitcher({
  mode,
  sessions,
  focusedChannelId,
  onClose,
  onPick,
}: {
  mode: SwitcherMode
  sessions: Session[]
  focusedChannelId: string
  onClose: () => void
  onPick: (pick: Pick, split: boolean) => void
}) {
  const [query, setQuery] = useState("")
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const restoreRef = useRef<HTMLElement | null>(null)
  const now = useNow(60_000)
  const channel = channelById(focusedChannelId)

  // Modal: focus stays in the field, even when a menu that just closed hands
  // focus back to its trigger a frame after the switcher opened.
  useEffect(() => {
    restoreRef.current = document.activeElement as HTMLElement | null
    inputRef.current?.focus()
    const keep = (event: FocusEvent) => {
      if (!dialogRef.current?.contains(event.target as Node)) inputRef.current?.focus()
    }
    document.addEventListener("focusin", keep)
    return () => {
      document.removeEventListener("focusin", keep)
      restoreRef.current?.focus?.()
    }
  }, [])

  const rows = useMemo<Row[]>(() => {
    const sessionRow = (session: Session, group: string, hits: number[] = []): Row => {
      const inChannel = channelById(session.channelId)
      return {
        key: `s-${session.id}`,
        group,
        pick: { kind: "session", id: session.id },
        render: (
          <>
            <StatusGlyph status={session.status} />
            <span className="spike-two-result-title">
              <Highlight text={session.title} hits={hits} />
            </span>
            <span className="spike-two-result-meta">
              #{inChannel?.name} · {agents[session.agent].label}
            </span>
            <span className="spike-two-result-trail">{ago(session.at, now)}</span>
          </>
        ),
      }
    }
    const channelRow = (target: Channel, hits: number[] = []): Row => ({
      key: `c-${target.id}`,
      group: "Channels",
      pick: { kind: "channel", id: target.id },
      render: (
        <>
          <span className="spike-two-result-icon">
            <DesktopIcon name={target.private ? "privateChannel" : "channel"} />
          </span>
          <span className="spike-two-result-title">
            <Highlight text={target.name} hits={hits} />
          </span>
          <span className="spike-two-result-meta">{target.topic}</span>
        </>
      ),
    })
    const written = sessions.filter((session) => session.messages.length > 0)
    const trimmed = query.trim()

    if (!trimmed) {
      const waiting = written.filter((session) => session.status === "approval")
      const recent = written
        .filter((session) => session.status !== "approval")
        .sort(byRecency)
        .slice(0, 6)
      return [
        {
          key: "new",
          group: "",
          pick: { kind: "new", channelId: focusedChannelId },
          render: (
            <>
              <span className="spike-two-result-icon">
                <DesktopIcon name="newSession" />
              </span>
              <span className="spike-two-result-title">
                New session in #{channel?.name}
              </span>
              <span className="spike-two-result-trail">
                <Kbd>{keys("⌘N")}</Kbd>
              </span>
            </>
          ),
        },
        ...waiting.map((session) => sessionRow(session, "Needs you")),
        ...recent.map((session) => sessionRow(session, "Recent")),
      ]
    }

    const sessionMatches = written
      .map((session) => {
        const title = fuzzy(trimmed, session.title)
        const context = fuzzy(
          trimmed,
          `${session.title} ${channelById(session.channelId)?.name ?? ""} ${agents[session.agent].label}`,
        )
        const best = title ?? context
        if (!best) return null
        return {
          session,
          score:
            (title ? title.score + 2 : (context?.score ?? 0)) +
            (session.status !== "idle" ? 1 : 0),
          hits: title ? title.hits : [],
        }
      })
      .filter((match) => match !== null)
      .sort((a, b) => b.score - a.score)
      .slice(0, 7)

    const channelMatches = channels
      .map((target) => ({ target, match: fuzzy(trimmed, target.name) }))
      .filter((entry) => entry.match !== null)
      .sort((a, b) => (b.match?.score ?? 0) - (a.match?.score ?? 0))
      .slice(0, 4)

    return [
      ...sessionMatches.map((match) => sessionRow(match.session, "Sessions", match.hits)),
      ...channelMatches.map((entry) => channelRow(entry.target, entry.match?.hits)),
      {
        key: "ask",
        group: "Start",
        pick: { kind: "new", channelId: focusedChannelId, text: trimmed },
        render: (
          <>
            <span className="spike-two-result-icon">
              <DesktopIcon name="enter" />
            </span>
            <span className="spike-two-result-title">“{trimmed}”</span>
            <span className="spike-two-result-meta">new session in #{channel?.name}</span>
          </>
        ),
      },
    ]
  }, [query, sessions, focusedChannelId, now, channel?.name])

  const clamped = Math.min(active, rows.length - 1)

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${clamped}"]`)
      ?.scrollIntoView({ block: "nearest" })
  }, [clamped])

  const onKeyDown = (event: ReactKeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault()
      onClose()
    } else if (event.key === "ArrowDown" || (event.ctrlKey && event.key === "n")) {
      event.preventDefault()
      setActive((clamped + 1) % rows.length)
    } else if (event.key === "ArrowUp" || (event.ctrlKey && event.key === "p")) {
      event.preventDefault()
      setActive((clamped - 1 + rows.length) % rows.length)
    } else if (event.key === "Enter") {
      event.preventDefault()
      const row = rows[clamped]
      if (row) onPick(row.pick, modKey(event))
    } else if (event.key === "Tab") {
      event.preventDefault()
    }
  }

  let lastGroup: string | null = null

  return (
    <div
      className="spike-two-overlay"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        className="spike-two-switcher"
        role="dialog"
        aria-modal="true"
        aria-label={mode === "split" ? "Open beside" : "Jump to"}
        onKeyDown={onKeyDown}
      >
        <div className="spike-two-switcher-field">
          {mode === "split" ? (
            <DesktopIcon name="splitRight" />
          ) : (
            <DesktopIcon name="search" />
          )}
          <input
            ref={inputRef}
            role="combobox"
            aria-expanded="true"
            aria-controls="spike-two-results"
            aria-activedescendant={
              rows[clamped] ? `spike-two-result-${clamped}` : undefined
            }
            aria-autocomplete="list"
            placeholder={
              mode === "split"
                ? "Open beside the current pane…"
                : "Jump to a session or channel…"
            }
            value={query}
            spellCheck={false}
            onChange={(event) => {
              setQuery(event.target.value)
              setActive(0)
            }}
          />
        </div>
        <div
          className="spike-two-results"
          id="spike-two-results"
          role="listbox"
          ref={listRef}
        >
          {rows.map((row, index) => {
            const heading = row.group && row.group !== lastGroup ? row.group : null
            lastGroup = row.group
            return (
              <Fragment key={row.key}>
                {heading ? (
                  <div className="spike-two-results-group" role="presentation">
                    {heading}
                  </div>
                ) : null}
                <div
                  id={`spike-two-result-${index}`}
                  role="option"
                  aria-selected={index === clamped}
                  data-index={index}
                  className="spike-two-result"
                  onPointerMove={() => index !== clamped && setActive(index)}
                  onClick={(event) => onPick(row.pick, modKey(event))}
                >
                  {row.render}
                </div>
              </Fragment>
            )
          })}
        </div>
        <div className="spike-two-switcher-foot" aria-hidden="true">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> to move
          </span>
          <span>
            <Kbd>↵</Kbd> {mode === "split" ? "open beside" : "open"}
          </span>
          {mode === "open" ? (
            <span>
              <Kbd>{keys("⌘")}</Kbd>
              <Kbd>↵</Kbd> open beside
            </span>
          ) : null}
          <span>
            <Kbd>esc</Kbd> close
          </span>
        </div>
      </div>
    </div>
  )
}
