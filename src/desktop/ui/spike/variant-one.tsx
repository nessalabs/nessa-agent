import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
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
import type { HostKind } from "../../../host/features"
import { AgentMark } from "../../../onboarding/ui/agent-mark"
import type { AgentId } from "../../../onboarding/model/onboarding"
import { useThemePreference } from "../../adapters/theme-preference"
import { agentForProvider, composerModels } from "../../model/composer-options"
import { Composer } from "../composer"
import { HeaderArt } from "../header-art"
import { DesktopIcon } from "../icons"
import { openSettings } from "../settings/settings-view"
import { ThemeMenu } from "../theme-menu"
import "./variant-one.css"

/* ------------------------------------------------------------------ */
/* Mock workspace                                                      */
/* ------------------------------------------------------------------ */

type Status = "running" | "needs" | "idle"

interface Channel {
  id: string
  name: string
  unread?: boolean
}

interface Section {
  id: string
  name: string
  channels: Channel[]
}

type Turn =
  | { kind: "user"; text: string }
  | { kind: "agent"; text: string }
  | { kind: "tool"; tool: "read" | "run" | "edit"; label: string; detail?: string }
  | { kind: "approval"; command: string; reason: string }

/** A model in the SDK catalog, as the composer names it. */
interface ModelRef {
  provider: string
  modelId: string
}

interface Session {
  id: string
  channelId: string
  title: string
  /** The model the session runs on; its provider decides the agent. */
  model: ModelRef
  status: Status
  time: string
  /** When the conversation began, as the heading says it; derived from `time` if absent. */
  started?: string
  preview: string
  pinned?: boolean
  unread?: boolean
  fresh?: boolean
  /** The agent's last turn is still arriving. */
  streaming?: boolean
  turns: Turn[]
}

const sections: Section[] = [
  {
    id: "starred",
    name: "Starred",
    channels: [
      { id: "desktop-app", name: "desktop-app" },
      { id: "release", name: "release", unread: true },
    ],
  },
  {
    id: "labs",
    name: "Nessa Labs",
    channels: [
      { id: "gateway", name: "gateway", unread: true },
      { id: "sdk", name: "sdk" },
      { id: "design-system", name: "design-system" },
      { id: "onboarding", name: "onboarding" },
    ],
  },
  {
    id: "personal",
    name: "Personal",
    channels: [
      { id: "reading-list", name: "reading-list" },
      { id: "dotfiles", name: "dotfiles" },
    ],
  },
]

const models = {
  opus: { provider: "anthropic", modelId: "claude-opus-5" },
  sonnet: { provider: "anthropic", modelId: "claude-sonnet-5" },
  fable: { provider: "anthropic", modelId: "claude-fable-5-1" },
  astra: { provider: "openai", modelId: "gpt-6-astra" },
  sol: { provider: "openai", modelId: "gpt-5.6-sol" },
  minimax: { provider: "opencode", modelId: "opencode/minimax-m3" },
  pickle: { provider: "opencode", modelId: "opencode/big-pickle" },
} satisfies Record<string, ModelRef>

const agentOf = (session: Session): AgentId =>
  agentForProvider(session.model.provider) ?? "claude"

const modelName = (model: ModelRef) =>
  composerModels.find(
    (entry) => entry.provider === model.provider && entry.modelId === model.modelId,
  )?.displayName ?? model.modelId

const agentNames: Record<AgentId, string> = {
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
}

function transcript(prompt: string, reply: string, extra: Turn[] = []): Turn[] {
  return [{ kind: "user", text: prompt }, ...extra, { kind: "agent", text: reply }]
}

const seedSessions: Session[] = [
  {
    id: "s1",
    channelId: "desktop-app",
    title: "Split panes for the chat workspace",
    model: models.opus,
    status: "running",
    time: "now",
    started: "22m ago",
    preview: "Laying out panes as fractions so a third pane never squeezes the first.",
    pinned: true,
    turns: [
      {
        kind: "user",
        text: "Let the workspace hold two or three conversations side by side. Dragging a session onto the chat area should open it in a split.",
      },
      {
        kind: "tool",
        tool: "read",
        label: "Read",
        detail: "src/desktop/ui/desktop-app.tsx",
      },
      {
        kind: "tool",
        tool: "read",
        label: "Read",
        detail: "split-view/split-view-math.ts",
      },
      {
        kind: "agent",
        text: "The shell already owns one SplitView for the sidebars, so I'd keep panes in a second, nested group. Each pane stores a fraction of the chat area rather than a pixel width, which means resizing the window keeps their proportions and a new pane takes its share from the one it was dropped beside.",
      },
      {
        kind: "tool",
        tool: "edit",
        label: "Edited",
        detail: "src/desktop/ui/workspace-panes.tsx  +142 −8",
      },
      {
        kind: "user",
        text: "Good. Cap it at three — past that nothing is readable at 1440.",
      },
      {
        kind: "agent",
        text: "Capped at three. Dropping a fourth replaces the pane under the pointer instead, and the drop target says so before you let go.",
      },
    ],
  },
  {
    id: "s2",
    channelId: "desktop-app",
    title: "Release build signing fails on CI",
    model: models.astra,
    status: "needs",
    time: "4m",
    started: "1h ago",
    preview: "Wants to run the notarisation step with the release keychain.",
    unread: true,
    turns: [
      ...transcript(
        "The macOS job fails at codesign since yesterday. Find out why.",
        "The keychain the job unlocks expired its certificate on the 25th. The new one is already in the vault; the job just needs to import it before signing.",
        [
          {
            kind: "tool",
            tool: "run",
            label: "Ran",
            detail: "gh run view 1184 --log-failed",
          },
          {
            kind: "tool",
            tool: "read",
            label: "Read",
            detail: ".github/workflows/release.yml",
          },
        ],
      ),
      {
        kind: "approval",
        command: "security import build/nessa-dev.p12 -k release.keychain",
        reason: "Imports a signing certificate into the release keychain.",
      },
    ],
  },
  {
    id: "s3",
    channelId: "desktop-app",
    title: "Frosted sidebar loses blur mid-slide",
    model: models.sonnet,
    status: "idle",
    time: "1h",
    preview:
      "Opacity below 1 made the peek its own backdrop root. Fixed by sliding instead.",
    pinned: true,
    turns: transcript(
      "When the sidebar peeks in, the blur switches off for a few frames. Why?",
      "An ancestor with opacity below 1 becomes the backdrop root for the glass inside it, so while the peek faded in, the blur had nothing behind it to sample. It now slides fully opaque, like the Dock, and only turns invisible once it is off-screen.",
      [{ kind: "tool", tool: "read", label: "Read", detail: "src/desktop/styles.css" }],
    ),
  },
  {
    id: "s4",
    channelId: "desktop-app",
    title: "Composer page mode threshold",
    model: models.minimax,
    status: "idle",
    time: "Yesterday",
    preview: "Seven lines in, three lines out, so the page never flickers at the edge.",
    turns: transcript(
      "The composer flips to page mode and back while typing around six lines.",
      "It used one threshold both ways. I gave it hysteresis: it becomes a page at seven lines and only returns to a card below three.",
    ),
  },
  {
    id: "s5",
    channelId: "desktop-app",
    title: "Night scene on 4K displays",
    model: models.opus,
    status: "idle",
    time: "Mon",
    preview:
      "Caps the scene at 290px so large windows get a finer band, not a coarser one.",
    turns: transcript(
      "The rain looks chunky on the Studio Display.",
      "The scene scaled with the window's height. It now stops growing at 290px and crops sideways instead, so a big window shows more of the street at the same fineness.",
    ),
  },
  {
    id: "s6",
    channelId: "release",
    title: "Draft 0.9 release notes",
    model: models.sonnet,
    status: "running",
    time: "2m",
    started: "9m ago",
    preview: "Grouping 41 merged PRs into what someone upgrading would notice first.",
    unread: true,
    turns: transcript(
      "Write release notes for 0.9 from everything merged since 0.8.3.",
      "Reading the merged PRs now. I'm grouping them by what someone upgrading would notice — the desktop window first, then the gateway, then fixes.",
      [
        {
          kind: "tool",
          tool: "run",
          label: "Ran",
          detail: "git log v0.8.3..main --merges",
        },
      ],
    ),
  },
  {
    id: "s7",
    channelId: "release",
    title: "Bump Tauri to 2.4",
    model: models.sol,
    status: "idle",
    time: "3h",
    preview: "Upgraded; the window-state plugin changed its config key.",
    turns: transcript(
      "Bump Tauri to 2.4 and fix whatever breaks.",
      "Done. The only break was the window-state plugin renaming its config key, which I updated in tauri.conf.json.",
    ),
  },
  {
    id: "s8",
    channelId: "gateway",
    title: "Reconnect storm after sleep",
    model: models.astra,
    status: "needs",
    time: "12m",
    started: "40m ago",
    preview: "Asks to restart the local gateway to test jittered backoff.",
    unread: true,
    turns: [
      ...transcript(
        "After the Mac wakes, every client reconnects at once and the gateway drops half of them.",
        "They all retry on the same fixed schedule. I added full jitter to the backoff; to prove it I need to restart the local gateway with 200 simulated clients.",
      ),
      {
        kind: "approval",
        command: "cargo run -p nessa-gateway -- --simulate-clients 200",
        reason: "Restarts the local gateway and opens 200 connections.",
      },
    ],
  },
  {
    id: "s9",
    channelId: "gateway",
    title: "Trace IDs across the ACP boundary",
    model: models.fable,
    status: "running",
    time: "6m",
    started: "2h ago",
    preview: "Threading the span context through session/prompt.",
    pinned: true,
    turns: transcript(
      "I want one trace from the panel click to the agent's first token.",
      "Threading the span context through `session/prompt` as a `_meta` field, so the agent side can pick it up without a protocol change.",
    ),
  },
  {
    id: "s10",
    channelId: "gateway",
    title: "Rate limit headers",
    model: models.pickle,
    status: "idle",
    time: "Tue",
    preview: "Surfaces retry-after in the error the panel shows.",
    turns: transcript(
      "Show when a rate limit resets instead of a generic error.",
      "The gateway now forwards retry-after, and the panel says ‘Try again in 40 seconds’.",
    ),
  },
  {
    id: "s11",
    channelId: "sdk",
    title: "Document session lifecycle",
    model: models.sonnet,
    status: "idle",
    time: "Yesterday",
    preview: "Each public call now states what it does to a closed session.",
    turns: transcript(
      "The SDK docs never say what happens if you prompt a closed session.",
      "Every public call on Session now documents its behaviour after close, with a doctest for the error.",
    ),
  },
  {
    id: "s12",
    channelId: "design-system",
    title: "Chip focus ring",
    model: models.opus,
    status: "idle",
    time: "Sun",
    preview: "Focus takes the theme's edge light instead of system blue.",
    turns: transcript(
      "The chips flash blue when focused. It fights the theme.",
      "Focus now uses a soft halo in the theme's edge light, the same colour as the resize glow.",
    ),
  },
  {
    id: "s13",
    channelId: "onboarding",
    title: "Intro sound volume",
    model: models.sol,
    status: "idle",
    time: "Sep 19",
    preview: "Fades in over 1.2s and respects the system mute.",
    turns: transcript("The intro sound is loud.", "It now fades in over 1.2 seconds."),
  },
  {
    id: "s14",
    channelId: "reading-list",
    title: "Summarise the CRDT paper",
    model: models.opus,
    status: "idle",
    time: "Sat",
    preview: "Five ideas worth stealing, and the one that doesn't apply to us.",
    turns: transcript(
      "Summarise ‘Peritext’ for someone who knows Yjs.",
      "Five ideas worth stealing, mostly around how marks expand at their boundaries, and one — tombstone compaction — that doesn't apply to a single-writer draft.",
    ),
  },
  {
    id: "s15",
    channelId: "dotfiles",
    title: "Move zsh config to XDG",
    model: models.minimax,
    status: "idle",
    time: "Aug 30",
    preview: "Everything under ~/.config/zsh with a one-line ~/.zshenv.",
    turns: transcript(
      "Tidy my home directory.",
      "Moved zsh to ~/.config/zsh; ~/.zshenv is one line now.",
    ),
  },
]

const allChannels = sections.flatMap((section) => section.channels)
const channelName = (id: string) =>
  allChannels.find((channel) => channel.id === id)?.name ?? id

type View = { kind: "channel"; id: string } | { kind: "smart"; id: "needs" | "running" }

/* ------------------------------------------------------------------ */
/* Layout: columns side by side, each holding one or more stacked panes */
/* ------------------------------------------------------------------ */

type Side = "left" | "right" | "top" | "bottom"
type Zone = Side | "center"
type Direction = "left" | "right" | "up" | "down"

/** A pane: one session, and its share of its column's height. */
interface Cell {
  key: number
  sessionId: string
  grow: number
}

/** A column: its share of the chat area's width, and its panes top to bottom. */
interface Column {
  key: number
  grow: number
  cells: Cell[]
}

type Layout = Column[]

const MAX_PANES = 4
const MAX_COLUMNS = 3
const PANE_MIN = 300
const ROW_MIN = 220

const cellsOf = (layout: Layout) => layout.flatMap((column) => column.cells)
const sum = (values: number[]) => values.reduce((a, b) => a + b, 0)
const clamp = (value: number, min: number, max: number) =>
  Math.min(Math.max(value, min), max)

function locate(layout: Layout, key: number) {
  for (let c = 0; c < layout.length; c++) {
    const r = layout[c].cells.findIndex((cell) => cell.key === key)
    if (r >= 0) return { c, r }
  }
  return null
}

/** Takes a pane out; the neighbour that was beside it takes its space. */
function removeCell(layout: Layout, key: number): Layout {
  const at = locate(layout, key)
  if (!at) return layout
  const column = layout[at.c]
  if (column.cells.length === 1) {
    const rest = layout.filter((_, i) => i !== at.c)
    const heir = Math.min(at.c, rest.length - 1)
    return rest.map((col, i) =>
      i === heir ? { ...col, grow: col.grow + column.grow } : col,
    )
  }
  const freed = column.cells[at.r].grow
  const cells = column.cells.filter((_, i) => i !== at.r)
  const heir = Math.min(at.r, cells.length - 1)
  return layout.map((col, i) =>
    i !== at.c
      ? col
      : {
          ...col,
          cells: cells.map((cell, j) =>
            j === heir ? { ...cell, grow: cell.grow + freed } : cell,
          ),
        },
  )
}

/**
 * Puts a pane beside `target`: left or right opens a full-height column that
 * takes half the target's column; top or bottom halves the target pane.
 */
function insertCell(
  layout: Layout,
  target: number,
  side: Side,
  cell: { key: number; sessionId: string },
): Layout {
  const at = locate(layout, target)
  if (!at) return layout
  if (side === "left" || side === "right") {
    const half = layout[at.c].grow / 2
    const column: Column = { key: cell.key, grow: half, cells: [{ ...cell, grow: 1 }] }
    const next = layout.map((col, i) => (i === at.c ? { ...col, grow: half } : col))
    const index = side === "right" ? at.c + 1 : at.c
    return [...next.slice(0, index), column, ...next.slice(index)]
  }
  const column = layout[at.c]
  const half = column.cells[at.r].grow / 2
  const cells = column.cells.map((c, j) => (j === at.r ? { ...c, grow: half } : c))
  const index = side === "bottom" ? at.r + 1 : at.r
  const placed = [
    ...cells.slice(0, index),
    { ...cell, grow: half },
    ...cells.slice(index),
  ]
  return layout.map((col, i) => (i === at.c ? { ...col, cells: placed } : col))
}

/** Two panes trade places; each place keeps its size. */
function swapCells(layout: Layout, a: number, b: number): Layout {
  const A = locate(layout, a)
  const B = locate(layout, b)
  if (!A || !B) return layout
  const cellA = layout[A.c].cells[A.r]
  const cellB = layout[B.c].cells[B.r]
  return layout.map((col, c) => ({
    ...col,
    cells: col.cells.map((cell, r) =>
      c === A.c && r === A.r
        ? { ...cellB, grow: cell.grow }
        : c === B.c && r === B.r
          ? { ...cellA, grow: cell.grow }
          : cell,
    ),
  }))
}

function moveCell(layout: Layout, key: number, target: number, zone: Zone): Layout {
  if (key === target) return layout
  if (zone === "center") return swapCells(layout, key, target)
  const at = locate(layout, key)
  if (!at) return layout
  return insertCell(removeCell(layout, key), target, zone, layout[at.c].cells[at.r])
}

function showIn(layout: Layout, key: number, sessionId: string): Layout {
  return layout.map((col) => ({
    ...col,
    cells: col.cells.map((cell) => (cell.key === key ? { ...cell, sessionId } : cell)),
  }))
}

/** Where each pane sits, as fractions the stylesheet turns into a rectangle. */
function placements(layout: Layout) {
  const width = sum(layout.map((col) => col.grow))
  const out: { cell: Cell; corner: boolean; style: React.CSSProperties }[] = []
  const edges: {
    key: string
    axis: "x" | "y"
    c: number
    r: number
    style: React.CSSProperties
  }[] = []
  let x = 0
  layout.forEach((column, c) => {
    const w = column.grow / width
    const height = sum(column.cells.map((cell) => cell.grow))
    let y = 0
    column.cells.forEach((cell, r) => {
      const h = cell.grow / height
      out.push({
        cell,
        corner: c === 0 && r === 0,
        style: {
          "--cx": x,
          "--cw": w,
          "--ci": c,
          "--cn": layout.length,
          "--ry": y,
          "--rh": h,
          "--ri": r,
          "--rn": column.cells.length,
        } as React.CSSProperties,
      })
      y += h
      if (r < column.cells.length - 1)
        edges.push({
          key: `r${column.key}-${r}`,
          axis: "y",
          c,
          r,
          style: {
            "--cx": x,
            "--cw": w,
            "--ci": c,
            "--cn": layout.length,
            "--ry": y,
            "--ri": r,
            "--rn": column.cells.length,
          } as React.CSSProperties,
        })
    })
    x += w
    if (c < layout.length - 1)
      edges.push({
        key: `c${column.key}`,
        axis: "x",
        c,
        r: 0,
        style: { "--cx": x, "--ci": c, "--cn": layout.length } as React.CSSProperties,
      })
  })
  return { panes: out, edges }
}
const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)
const mod = (event: { metaKey: boolean; ctrlKey: boolean }) =>
  isMac ? event.metaKey : event.ctrlKey
const key = (keys: string) =>
  isMac ? keys : keys.replace("⌥", "Alt+").replace("⌘", "Ctrl+")
const DRAG_TYPE = "application/x-nessa-session"
const PANE_TYPE = "application/x-nessa-pane"

/** A lightly underdamped spring (ζ 0.8), overshooting by about 1.5%. */
const SPRING =
  "linear(0, 0.0261, 0.0914, 0.1801, 0.2803, 0.3834, 0.4834, 0.5763, 0.66, 0.7333, 0.796, 0.8485, 0.8914, 0.9258, 0.9527, 0.9732, 0.9884, 0.9993, 1.0067, 1.0114, 1.014, 1.0151, 1.0151, 1.0143, 1.0131, 1.0117, 1.0101, 1.0086, 1.0071, 1.0057, 1.0046, 1.0035, 1)"
const EASE = "cubic-bezier(0.32, 0.72, 0, 1)"

const reducedMotion = () =>
  typeof matchMedia !== "undefined" &&
  matchMedia("(prefers-reduced-motion: reduce)").matches

/** "started 22m ago"; `lead` capitalises it when nothing comes before it. */
function startedLabel(session: Session, lead = false) {
  const { time } = session
  const when = session.started
    ? session.started
    : time === "now"
      ? "just now"
      : /^\d+[mh]$/.test(time)
        ? `${time} ago`
        : time === "Yesterday"
          ? "yesterday"
          : time
  return `${lead ? "Started" : "started"} ${when}`
}

/** How long a new session's home takes to hand over to its conversation. */
const ARRIVAL_MS = 460

/** Whether two lines say the same thing, ignoring case, spacing and end punctuation. */
function sameWords(a: string, b: string) {
  const plain = (text: string) =>
    text
      .trim()
      .toLowerCase()
      .replace(/[.?!:,;…]+$/, "")
      .replace(/\s+/g, " ")
  return plain(a) === plain(b)
}

/** A session's title from its first message: the first sentence, kept short. */
function titleFrom(text: string) {
  const first = text
    .trim()
    .split(/\n|(?<=[.?!])\s/)[0]
    .replace(/[.?!:,;]+$/, "")
  const short =
    first.length <= 48 ? first : `${first.slice(0, 48).replace(/\s+\S*$/, "")}…`
  return short.charAt(0).toUpperCase() + short.slice(1)
}

function mockReply(text: string, channel: string) {
  const ask = text.trim().replace(/[.?!]+$/, "")
  const topic =
    ask.length > 60 ? "this" : `“${ask.charAt(0).toLowerCase()}${ask.slice(1)}”`
  return `On it. I’ll start by reading what #${channel} already has around ${topic}, then come back with a short plan before I change anything. If it touches more than a couple of files, I’ll ask first.`
}

/**
 * A compact pill for the drag, in place of the browser's translucent copy of
 * the whole row, which overlaps the rows around it.
 */
function setDragGhost(event: DragEvent<HTMLElement>, title: string) {
  const row = event.currentTarget
  const ghost = document.createElement("div")
  ghost.className = "spike-one-ghost"
  const tile = row.querySelector(".spike-one-agent")?.cloneNode(true)
  if (tile) ghost.append(tile)
  const label = document.createElement("span")
  label.textContent = title
  ghost.append(label)
  ;(row.closest(".spike-one") ?? document.body).append(ghost)
  event.dataTransfer.setDragImage(ghost, 18, 17)
  // The image is captured synchronously; the node is only needed this frame.
  requestAnimationFrame(() => ghost.remove())
}

function startDrag(event: DragEvent<HTMLElement>, session: Session) {
  event.dataTransfer.setData(DRAG_TYPE, session.id)
  event.dataTransfer.effectAllowed = "copy"
  setDragGhost(event, session.title)
  // The row stays put, a little dimmed, while its pill travels.
  const row = event.currentTarget
  row.dataset.dragging = ""
  row.addEventListener("dragend", () => delete row.dataset.dragging, { once: true })
}

/**
 * FLIP with scale correction: the pane's box travels and resizes from `from`
 * to `to` by transform, while its contents counter-scale on the same curve,
 * so the text, laid out once at its final size, never stretches. Box and
 * contents scale about the pane's centre, so what is centred in the pane
 * (the transcript, the composer) glides from its old centre to its new one
 * rather than starting off to one side.
 */
function flyPane(pane: HTMLElement, from: DOMRect, to: DOMRect): Animation[] {
  const steps = 14
  const ease = (t: number) => 1 - Math.pow(1 - t, 3.2)
  const dx = from.left + from.width / 2 - (to.left + to.width / 2)
  const dy = from.top + from.height / 2 - (to.top + to.height / 2)
  const sx = from.width / to.width
  const sy = from.height / to.height
  const box: Keyframe[] = []
  const inner: Keyframe[] = []
  for (let i = 0; i <= steps; i++) {
    const e = ease(i / steps)
    const x = sx + (1 - sx) * e
    const y = sy + (1 - sy) * e
    box.push({
      transform: `translate(${dx * (1 - e)}px, ${dy * (1 - e)}px) scale(${x}, ${y})`,
    })
    inner.push({ transform: `scale(${1 / x}, ${1 / y})` })
  }
  const timing: KeyframeAnimationOptions = { duration: 220, easing: "linear" }
  pane.style.transformOrigin = "50% 50%"
  return [
    pane.animate(box, timing),
    ...Array.from(pane.children, (child) => {
      // The pane's centre, in the child's own box.
      const offset = (child as HTMLElement).offsetTop
      ;(child as HTMLElement).style.transformOrigin =
        `${to.width / 2}px ${to.height / 2 - offset}px`
      return child.animate(inner, timing)
    }),
  ]
}

/* ------------------------------------------------------------------ */
/* Small pieces                                                        */
/* ------------------------------------------------------------------ */

/** Agents the design system ships a mark for; the rest get a quiet monogram. */
const shippedMarks: Record<AgentId, boolean> = {
  claude: true,
  codex: true,
  opencode: false,
}

function AgentTile({ agent, size = 20 }: { agent: AgentId; size?: number }) {
  return (
    <span
      className="spike-one-agent"
      data-agent={agent}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.5) }}
    >
      {shippedMarks[agent] ? (
        <AgentMark id={agent} name={agentNames[agent]} />
      ) : (
        <span className="spike-one-monogram" aria-hidden="true">
          {agentNames[agent].slice(0, 1)}
        </span>
      )}
    </span>
  )
}

/** Needs you is a lit amber dot; running is a small turning arc, never a bare ring. */
function StatusDot({ status }: { status: Status }) {
  if (status === "idle") return null
  const label = status === "running" ? "Working" : "Needs you"
  return (
    <span
      className="spike-one-dot"
      data-status={status}
      role="img"
      aria-label={label}
      title={label}
    />
  )
}

/** Drags a boundary; reports the pointer's travel along `axis` since the press. */
function useDrag(axis: "x" | "y", onStart: () => void, onMove: (delta: number) => void) {
  const start = useRef<number | null>(null)
  const at = (event: ReactPointerEvent<HTMLElement>) =>
    axis === "x" ? event.clientX : event.clientY
  return {
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => {
      event.preventDefault()
      onStart()
      event.currentTarget.setPointerCapture(event.pointerId)
      start.current = at(event)
      document.documentElement.dataset.spikeOneResizing = axis
    },
    onPointerMove: (event: ReactPointerEvent<HTMLElement>) => {
      if (start.current === null) return
      onMove(at(event) - start.current)
    },
    onPointerUp: (event: ReactPointerEvent<HTMLElement>) => {
      if (start.current === null) return
      event.currentTarget.releasePointerCapture(event.pointerId)
      start.current = null
      delete document.documentElement.dataset.spikeOneResizing
    },
  }
}

function Edge({
  label,
  axis = "x",
  style,
  onStart,
  onMove,
  onKey,
}: {
  label: string
  /** The direction the edge moves in: "x" for a vertical edge, "y" for a horizontal one. */
  axis?: "x" | "y"
  style?: React.CSSProperties
  onStart: () => void
  onMove: (delta: number) => void
  onKey: (step: number) => void
}) {
  const drag = useDrag(axis, onStart, onMove)
  // The glow follows the pointer along the edge's length.
  const glow = (event: ReactPointerEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect()
    const along = axis === "x" ? event.clientY - box.top : event.clientX - box.left
    event.currentTarget.style.setProperty("--spike-one-glow", `${along}px`)
  }
  const [back, forward] =
    axis === "x" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"]
  return (
    <div
      role="separator"
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-label={label}
      tabIndex={0}
      className="spike-one-edge"
      data-axis={axis}
      style={style}
      {...drag}
      onPointerMove={(event) => {
        glow(event)
        drag.onPointerMove(event)
      }}
      onPointerEnter={glow}
      onKeyDown={(event) => {
        if (event.key === back) onKey(-16)
        else if (event.key === forward) onKey(16)
        else return
        event.preventDefault()
      }}
    />
  )
}

/* ------------------------------------------------------------------ */
/* Root                                                                */
/* ------------------------------------------------------------------ */

/** Spike: workspace variation 1 — a Mail/Finder-style three-column window. */
export function VariantOne({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const [theme, setTheme] = useThemePreference()
  const [sessions, setSessions] = useState(seedSessions)
  const [view, setView] = useState<View>({ kind: "channel", id: "desktop-app" })
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const [unreadChannels, setUnreadChannels] = useState(
    () => new Set(allChannels.filter((c) => c.unread).map((c) => c.id)),
  )
  const [layout, setLayout] = useState<Layout>([
    { key: 0, grow: 1, cells: [{ key: 1, sessionId: "s1", grow: 1 }] },
  ])
  const [focusedKey, setFocusedKey] = useState(1)
  const [draggingPane, setDraggingPane] = useState<number | null>(null)
  const [sidebarOpen, setSidebarOpen] = useState(true)
  const [listOpen, setListOpen] = useState(true)
  const [sidebarWidth, setSidebarWidth] = useState(240)
  const [listWidth, setListWidth] = useState(312)
  const [query, setQuery] = useState("")
  const nextKey = useRef(2)
  const searchRef = useRef<HTMLInputElement>(null)
  const chatRef = useRef<HTMLDivElement>(null)

  const byId = useMemo(() => new Map(sessions.map((s) => [s.id, s])), [sessions])

  // FLIP: where every pane (and the list, beside them) was, taken just
  // before a change of layout or a column folding, so each can travel from
  // there by transform once the new layout has landed. Nothing animates its
  // size, so no frame of the motion rewraps text.
  const before = useRef<{
    panes: Map<number, DOMRect>
    list?: DOMRect
    title?: DOMRect
  } | null>(null)
  const flights = useRef<Animation[]>([])
  const shellRef = useRef<HTMLDivElement>(null)
  const snapshot = useCallback(() => {
    const panes = new Map<number, DOMRect>()
    chatRef.current
      ?.querySelectorAll<HTMLElement>("[data-pane-key]")
      .forEach((pane) =>
        panes.set(Number(pane.dataset.paneKey), pane.getBoundingClientRect()),
      )
    const shell = shellRef.current
    const list = shell?.querySelector(".spike-one-list-inner")?.getBoundingClientRect()
    const title = shell?.querySelector(".spike-one-list-title")?.getBoundingClientRect()
    before.current = { panes, list, title }
  }, [])
  /** Folds or unfolds a column, with everything beside it travelling to its new place. */
  const toggleSidebar = useCallback(
    (open?: boolean) => {
      snapshot()
      setSidebarOpen((value) => open ?? !value)
    },
    [snapshot],
  )
  const toggleList = useCallback(
    (open?: boolean) => {
      snapshot()
      setListOpen((value) => open ?? !value)
    },
    [snapshot],
  )
  useLayoutEffect(() => {
    const from = before.current
    const root = chatRef.current
    before.current = null
    if (!from || !root || reducedMotion()) return
    // A change during a flight starts from where the panes are now (the
    // snapshot read them mid-flight), so the old flight simply stops.
    flights.current.forEach((flight) => flight.cancel())
    const flying: Animation[] = []
    // The list slides sideways when the sidebar folds; its heading's shift
    // past the window controls rides along inside it.
    const slide = (element: HTMLElement, dx: number) =>
      Math.abs(dx) >= 0.5 &&
      flying.push(
        element.animate([{ transform: `translateX(${dx}px)` }, { transform: "none" }], {
          duration: 240,
          easing: EASE,
        }),
      )
    const shell = shellRef.current
    const list = shell?.querySelector<HTMLElement>(".spike-one-list-inner")
    const title = shell?.querySelector<HTMLElement>(".spike-one-list-title")
    if (listOpen && list && from.list) {
      const dx = from.list.left - list.getBoundingClientRect().left
      slide(list, dx)
      if (title && from.title)
        slide(title, from.title.left - title.getBoundingClientRect().left - dx)
    }
    root.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane) => {
      const a = from.panes.get(Number(pane.dataset.paneKey))
      if (!a) return
      const b = pane.getBoundingClientRect()
      const still =
        Math.abs(a.left - b.left) < 0.5 &&
        Math.abs(a.top - b.top) < 0.5 &&
        Math.abs(a.width - b.width) < 0.5 &&
        Math.abs(a.height - b.height) < 0.5
      if (still) return
      flying.push(...flyPane(pane, a, b))
    })
    flights.current = flying
    if (flying.length === 0) {
      delete root.dataset.flipping
      return
    }
    root.dataset.flipping = ""
    Promise.all(flying.map((flight) => flight.finished))
      .then(() => {
        if (flights.current === flying) delete root.dataset.flipping
      })
      .catch(() => undefined)
    // Runs on every change of layout or column, but only a snapshot taken
    // just before it starts a flight.
  }, [layout, sidebarOpen, listOpen])

  // A window made too narrow for its panes folds the columns away, the
  // sidebar first and then the list, as Mail does. Only a change of window
  // size does this, so a column opened by hand in a small window stays open.
  const fold = useRef({ sidebarOpen, listOpen, sidebarWidth, listWidth, columns: 1 })
  fold.current = {
    sidebarOpen,
    listOpen,
    sidebarWidth,
    listWidth,
    columns: layout.length,
  }
  useEffect(() => {
    const fit = () => {
      const f = fold.current
      const vw = window.innerWidth
      const need = f.columns * PANE_MIN + (f.columns - 1) * 8
      const sidebar = f.sidebarOpen
        ? Math.min(f.sidebarWidth, Math.max(200, vw * 0.2)) + 8
        : 0
      const list = f.listOpen ? Math.min(f.listWidth, Math.max(260, vw * 0.26)) + 8 : 0
      if (vw - 16 - sidebar - list >= need) return
      if (f.sidebarOpen) setSidebarOpen(false)
      if (f.listOpen && vw - 16 - list < need) setListOpen(false)
    }
    fit()
    window.addEventListener("resize", fit)
    return () => window.removeEventListener("resize", fit)
  }, [])

  // A new session nobody wrote in is let go once no pane shows it.
  useEffect(() => {
    const shown = new Set(cellsOf(layout).map((cell) => cell.sessionId))
    setSessions((all) =>
      all.some((s) => s.fresh && !shown.has(s.id))
        ? all.filter((s) => !s.fresh || shown.has(s.id))
        : all,
    )
  }, [layout])

  const cells = cellsOf(layout)
  const focusedCell = cells.find((cell) => cell.key === focusedKey) ?? cells[0]
  const focusedSession = byId.get(focusedCell?.sessionId ?? "")

  const markRead = useCallback((id: string) => {
    setSessions((all) =>
      all.map((s) => (s.id === id && s.unread ? { ...s, unread: false } : s)),
    )
  }, [])

  /** Opens a session in the focused pane, or focuses the pane already showing it. */
  const open = useCallback(
    (id: string) => {
      markRead(id)
      const existing = cellsOf(layout).find((cell) => cell.sessionId === id)
      if (existing) {
        setFocusedKey(existing.key)
        return
      }
      setLayout((all) => showIn(all, focusedCell.key, id))
    },
    [layout, focusedCell, markRead],
  )

  const paneBox = useCallback(
    (paneKey: number) =>
      chatRef.current
        ?.querySelector<HTMLElement>(`[data-pane-key="${paneKey}"]`)
        ?.getBoundingClientRect(),
    [],
  )

  /**
   * Whether a pane could go on `side` of `target`: a column needs room for a
   * readable width (the sidebar steps aside if that is what it takes), a row
   * needs half the target's height to still be readable.
   */
  const canPlace = useCallback(
    (side: Side, target: number, moving?: number) => {
      const base = moving === undefined ? layout : removeCell(layout, moving)
      if (moving === undefined && cellsOf(layout).length >= MAX_PANES) return false
      const box = paneBox(target)
      if (side === "left" || side === "right") {
        if (base.length >= MAX_COLUMNS) return false
        const spare = sidebarOpen ? sidebarWidth + 8 : 0
        return !box || (box.width + spare) / 2 >= PANE_MIN
      }
      return !box || box.height / 2 >= ROW_MIN
    },
    [layout, sidebarOpen, sidebarWidth, paneBox],
  )

  /** Before a new column: short of a readable width, the sidebar steps aside. */
  const makeRoom = useCallback(
    (target: number, side: Side) => {
      if (side !== "left" && side !== "right") return
      const box = paneBox(target)
      // The caller has taken the snapshot this fold travels from.
      if (box && box.width / 2 < PANE_MIN) setSidebarOpen(false)
    },
    [paneBox],
  )

  /**
   * Adds a pane on `side` of `target`. When a column won't fit it stacks
   * instead, and past four panes it replaces the target.
   */
  const split = useCallback(
    (
      id: string,
      target = focusedCell.key,
      side: Side = "right",
      /** Past four panes, whether the session takes the target's place. */
      replace = true,
    ) => {
      markRead(id)
      // A session already on screen is focused where it is, never shown twice.
      const existing = cellsOf(layout).find((cell) => cell.sessionId === id)
      if (existing) {
        setFocusedKey(existing.key)
        return false
      }
      let place: Side | null = side
      if (!canPlace(place, target)) {
        const other: Side = side === "left" || side === "right" ? "bottom" : "right"
        place = canPlace(other, target) ? other : null
      }
      if (!place) {
        if (!replace) return false
        setLayout((all) => showIn(all, target, id))
        setFocusedKey(target)
        return true
      }
      makeRoom(target, place)
      const cell = { key: nextKey.current++, sessionId: id }
      const at = place
      snapshot()
      setLayout((all) => insertCell(all, target, at, cell))
      setFocusedKey(cell.key)
      return true
    },
    [layout, focusedCell, canPlace, makeRoom, markRead, snapshot],
  )

  const closePane = useCallback(
    (paneKey: number) => {
      const order = cellsOf(layout)
      if (order.length === 1) return
      const index = order.findIndex((cell) => cell.key === paneKey)
      const heir = order[index + 1] ?? order[index - 1]
      snapshot()
      setLayout((all) => removeCell(all, paneKey))
      if (focusedKey === paneKey && heir) setFocusedKey(heir.key)
    },
    [layout, focusedKey, snapshot],
  )

  const movePane = useCallback(
    (paneKey: number, target: number, zone: Zone) => {
      if (zone !== "center") makeRoom(target, zone)
      snapshot()
      setLayout((all) => moveCell(all, paneKey, target, zone))
      setFocusedKey(paneKey)
    },
    [makeRoom, snapshot],
  )

  /**
   * ⌃⌥ and an arrow: trade places with the pane that way. At the side of a
   * stacked column, the pane steps out into a column of its own.
   */
  const nudge = useCallback(
    (paneKey: number, direction: Direction) => {
      const at = locate(layout, paneKey)
      if (!at) return
      const column = layout[at.c]
      if (direction === "up" || direction === "down") {
        const neighbour = column.cells[at.r + (direction === "up" ? -1 : 1)]
        if (neighbour) {
          snapshot()
          setLayout((all) => swapCells(all, paneKey, neighbour.key))
        }
        return
      }
      const beside = layout[at.c + (direction === "left" ? -1 : 1)]
      if (beside) {
        const neighbour = beside.cells[Math.min(at.r, beside.cells.length - 1)]
        snapshot()
        setLayout((all) => swapCells(all, paneKey, neighbour.key))
      } else if (column.cells.length > 1 && layout.length < MAX_COLUMNS) {
        const sibling = column.cells.find((cell) => cell.key !== paneKey)
        if (sibling) movePane(paneKey, sibling.key, direction)
      }
    },
    [layout, movePane, snapshot],
  )

  const newSession = useCallback(
    (inSplit?: Side, target = focusedCell.key) => {
      // Four panes is the most: a split asked for past that does nothing,
      // rather than putting a blank session over a conversation.
      if (inSplit && cellsOf(layout).length >= MAX_PANES) return
      const channelId = view.kind === "channel" ? view.id : "desktop-app"
      const id = `new-${Date.now()}`
      setSessions((all) => [
        {
          id,
          channelId,
          title: "New session",
          model: models.opus,
          status: "idle",
          time: "now",
          preview: "Nothing sent yet.",
          fresh: true,
          turns: [],
        },
        ...all,
      ])
      if (view.kind !== "channel") setView({ kind: "channel", id: channelId })
      if (inSplit) split(id, target, inSplit, false)
      else setLayout((all) => showIn(all, target, id))
    },
    [layout, view, split, focusedCell],
  )

  const selectChannel = (id: string) => {
    setView({ kind: "channel", id })
    setQuery("")
    // With the list hidden, choosing a channel still shows something of it:
    // its most pressing session opens in the focused pane.
    if (!listOpen) {
      const inChannel = sessions.filter((s) => s.channelId === id)
      const pick =
        inChannel.find((s) => s.status === "needs") ??
        inChannel.find((s) => s.status === "running") ??
        inChannel[0]
      if (pick) open(pick.id)
    }
    setUnreadChannels((all) => {
      if (!all.has(id)) return all
      const next = new Set(all)
      next.delete(id)
      return next
    })
  }

  const togglePin = (id: string) =>
    setSessions((all) => all.map((s) => (s.id === id ? { ...s, pinned: !s.pinned } : s)))

  const archive = (id: string) => {
    setSessions((all) => all.filter((s) => s.id !== id))
    const showing = cells.find((cell) => cell.sessionId === id)
    if (!showing) return
    // The last pane is never left showing nothing: it starts a new session.
    if (cells.length > 1) closePane(showing.key)
    else newSession(undefined, showing.key)
  }

  // Mock replies run on timers that die with the window.
  const timers = useRef(new Set<number>())
  useEffect(() => {
    const pending = timers.current
    return () => pending.forEach((timer) => clearTimeout(timer))
  }, [])
  const later = useCallback((run: () => void, ms: number) => {
    const timer = window.setTimeout(() => {
      timers.current.delete(timer)
      run()
    }, ms)
    timers.current.add(timer)
  }, [])

  const patch = useCallback(
    (id: string, change: (session: Session) => Partial<Session>) =>
      setSessions((all) => all.map((s) => (s.id === id ? { ...s, ...change(s) } : s))),
    [],
  )

  /** Streams a reply in a few words at a time, then lets the session rest. */
  const streamReply = useCallback(
    (id: string, reply: string) => {
      const words = reply.split(/(?<=\s)/)
      patch(id, (s) => ({
        streaming: true,
        turns: [...s.turns, { kind: "agent", text: "" }],
      }))
      const step = (shown: number) => {
        const next = Math.min(shown + 2, words.length)
        const done = next === words.length
        patch(id, (s) => ({
          turns: s.turns.map((turn, i) =>
            i === s.turns.length - 1 && turn.kind === "agent"
              ? { ...turn, text: words.slice(0, next).join("") }
              : turn,
          ),
          ...(done
            ? { streaming: false, status: "idle", time: "now", preview: reply }
            : {}),
        }))
        if (!done) later(() => step(next), 38)
      }
      later(() => step(0), 60)
    },
    [patch, later],
  )

  /** Sends a message: the first one also names the session and starts it. */
  const send = useCallback(
    (id: string, text: string) => {
      const session = sessions.find((s) => s.id === id)
      if (!session) return
      patch(id, (s) => ({
        ...(s.fresh ? { title: titleFrom(text), started: "just now" } : {}),
        fresh: false,
        status: "running",
        streaming: false,
        time: "now",
        preview: text,
        turns: [...s.turns, { kind: "user", text }],
      }))
      later(() => streamReply(id, mockReply(text, channelName(session.channelId))), 1400)
    },
    [sessions, patch, later, streamReply],
  )

  const approve = (id: string, allow: boolean) =>
    setSessions((all) =>
      all.map((s) =>
        s.id === id
          ? {
              ...s,
              status: allow ? "running" : "idle",
              time: "now",
              turns: [
                ...s.turns.filter((turn) => turn.kind !== "approval"),
                allow
                  ? {
                      kind: "tool",
                      tool: "run",
                      label: "Allowed once",
                      detail: s.turns.find((t) => t.kind === "approval")?.command,
                    }
                  : {
                      kind: "agent",
                      text: "Understood — I won’t run it. Anything else to try?",
                    },
              ],
            }
          : s,
      ),
    )

  // Window-level shortcuts, read through a ref so the listener is bound once.
  const keys = useRef({
    newSession,
    closePane,
    nudge,
    focusedCell,
    cells,
    toggleSidebar,
    toggleList,
    listOpen,
  })
  keys.current = {
    newSession,
    closePane,
    nudge,
    focusedCell,
    cells,
    toggleSidebar,
    toggleList,
    listOpen,
  }
  useEffect(() => {
    const arrows: Record<string, Direction> = {
      ArrowLeft: "left",
      ArrowRight: "right",
      ArrowUp: "up",
      ArrowDown: "down",
    }
    const onKeyDown = (event: KeyboardEvent) => {
      const k = keys.current
      // ⌃⌥ and an arrow moves the focused pane.
      if (
        event.ctrlKey &&
        event.altKey &&
        !event.metaKey &&
        Object.hasOwn(arrows, event.key)
      ) {
        k.nudge(k.focusedCell.key, arrows[event.key])
        event.preventDefault()
        return
      }
      if (!mod(event)) return
      if (event.code === "KeyB" && !event.altKey) {
        k.toggleSidebar()
      } else if (event.code === "KeyS" && event.altKey) {
        k.toggleList()
      } else if (event.code === "KeyN") {
        k.newSession(event.shiftKey ? "right" : undefined)
      } else if (event.code === "Backslash") {
        k.newSession(event.shiftKey ? "bottom" : "right")
      } else if (event.code === "KeyW" && k.cells.length > 1) {
        k.closePane(k.focusedCell.key)
      } else if (event.code === "KeyF" || event.code === "KeyK") {
        // Searching brings the list back first; it is inert while hidden.
        if (!k.listOpen) k.toggleList(true)
        requestAnimationFrame(() => searchRef.current?.focus())
      } else if (/^Digit[1-4]$/.test(event.code)) {
        const cell = k.cells[Number(event.code.slice(5)) - 1]
        if (cell) setFocusedKey(cell.key)
        else return
      } else return
      event.preventDefault()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])

  // Panes resize in shares of the chat area, so the window can grow or shrink
  // without the panes losing their proportions.
  const resizeFrom = useRef<Layout>(layout)
  const resizeColumns = (c: number, dx: number) => {
    const base = resizeFrom.current
    const room = (chatRef.current?.clientWidth ?? 1) - (base.length - 1) * 8
    const total = sum(base.map((col) => col.grow))
    const pair = base[c].grow + base[c + 1].grow
    const min = (PANE_MIN / room) * total
    const left = clamp(base[c].grow + (dx / room) * total, min, pair - min)
    setLayout(
      base.map((col, i) =>
        i === c
          ? { ...col, grow: left }
          : i === c + 1
            ? { ...col, grow: pair - left }
            : col,
      ),
    )
  }
  const resizeRows = (c: number, r: number, dy: number) => {
    const base = resizeFrom.current
    const column = base[c]
    const room = (chatRef.current?.clientHeight ?? 1) - (column.cells.length - 1) * 8
    const total = sum(column.cells.map((cell) => cell.grow))
    const pair = column.cells[r].grow + column.cells[r + 1].grow
    const min = (ROW_MIN / room) * total
    const top = clamp(column.cells[r].grow + (dy / room) * total, min, pair - min)
    setLayout(
      base.map((col, i) =>
        i !== c
          ? col
          : {
              ...col,
              cells: col.cells.map((cell, j) =>
                j === r
                  ? { ...cell, grow: top }
                  : j === r + 1
                    ? { ...cell, grow: pair - top }
                    : cell,
              ),
            },
      ),
    )
  }

  const sidebarFrom = useRef(sidebarWidth)
  const listFrom = useRef(listWidth)
  const listed = useMemo(() => {
    const q = query.trim().toLowerCase()
    return sessions.filter((s) => {
      // A new session joins the list with its first message.
      if (s.fresh) return false
      if (view.kind === "channel" && s.channelId !== view.id) return false
      if (view.kind === "smart" && s.status !== view.id) return false
      if (!q) return true
      return `${s.title} ${s.preview} ${modelName(s.model)} ${agentNames[agentOf(s)]}`
        .toLowerCase()
        .includes(q)
    })
  }, [sessions, view, query])

  const placed = placements(layout)
  const needsCount = sessions.filter((s) => s.status === "needs").length
  const runningCount = sessions.filter((s) => s.status === "running").length
  const title =
    view.kind === "channel"
      ? channelName(view.id)
      : view.id === "needs"
        ? "Needs you"
        : "Running"

  return (
    <div
      ref={shellRef}
      className="spike-one"
      data-host={hostKind}
      data-surface={browserSurface ? "browser" : "window"}
      data-desktop-theme={theme}
      data-sidebar={sidebarOpen ? "open" : "closed"}
      data-list={listOpen ? "open" : "closed"}
      style={
        {
          "--spike-one-sidebar": `${sidebarWidth}px`,
          "--spike-one-list": `${listWidth}px`,
        } as React.CSSProperties
      }
    >
      <div className="desktop-ambient" aria-hidden="true">
        <span className="desktop-grain" />
      </div>

      {/* The window's drag strip and its one stationary control. */}
      <div className="spike-one-titlebar" data-tauri-drag-region>
        <button
          type="button"
          className="spike-one-icon-button"
          aria-label={`${sidebarOpen ? "Hide" : "Show"} Sidebar (${key("⌘B")})`}
          title={`${sidebarOpen ? "Hide" : "Show"} Sidebar (${key("⌘B")})`}
          aria-expanded={sidebarOpen}
          onClick={() => toggleSidebar()}
        >
          <DesktopIcon name="sidebar" />
        </button>
        <button
          type="button"
          className="spike-one-icon-button"
          aria-label={`${listOpen ? "Hide" : "Show"} Session List (${key("⌥⌘S")})`}
          title={`${listOpen ? "Hide" : "Show"} Session List (${key("⌥⌘S")})`}
          aria-expanded={listOpen}
          onClick={() => toggleList()}
        >
          <DesktopIcon name="sessionList" />
        </button>
      </div>

      <nav
        className="spike-one-sidebar"
        aria-label="Channels"
        inert={!sidebarOpen}
        aria-hidden={!sidebarOpen}
      >
        <div className="spike-one-sidebar-scroll">
          <div className="spike-one-smart">
            <SidebarRow
              icon={<DesktopIcon name="needsYou" />}
              label="Needs you"
              active={view.kind === "smart" && view.id === "needs"}
              badge={needsCount || undefined}
              badgeTone="needs"
              onClick={() => setView({ kind: "smart", id: "needs" })}
            />
            <SidebarRow
              icon={<DesktopIcon name="running" />}
              label="Running"
              active={view.kind === "smart" && view.id === "running"}
              badge={runningCount || undefined}
              onClick={() => setView({ kind: "smart", id: "running" })}
            />
          </div>

          {sections.map((section) => {
            const shut = collapsed[section.id] ?? false
            return (
              <section
                key={section.id}
                className="spike-one-section"
                data-collapsed={shut || undefined}
              >
                <div className="spike-one-section-head">
                  <button
                    type="button"
                    className="spike-one-section-toggle"
                    aria-expanded={!shut}
                    onClick={() =>
                      setCollapsed((all) => ({ ...all, [section.id]: !shut }))
                    }
                  >
                    <span>{section.name}</span>
                    <DesktopIcon name="chevronRight" />
                  </button>
                  <button
                    type="button"
                    className="spike-one-section-add"
                    aria-label={`Add channel to ${section.name}`}
                    title="Add channel"
                  >
                    <DesktopIcon name="add" />
                  </button>
                </div>
                <div className="spike-one-section-body">
                  <ul role="list">
                    {section.channels.map((channel) => {
                      const active = view.kind === "channel" && view.id === channel.id
                      const inChannel = sessions.filter((s) => s.channelId === channel.id)
                      const running = inChannel.some((s) => s.status === "running")
                      const needs = inChannel.filter((s) => s.status === "needs").length
                      const pinned = inChannel.filter((s) => s.pinned)
                      return (
                        <li key={channel.id}>
                          <SidebarRow
                            icon={<DesktopIcon name="channel" />}
                            label={channel.name}
                            active={active}
                            unread={unreadChannels.has(channel.id)}
                            badge={needs || undefined}
                            badgeTone="needs"
                            running={running && !needs}
                            onClick={() => selectChannel(channel.id)}
                          />
                          {active && pinned.length > 0 ? (
                            <ul role="list" className="spike-one-pinned">
                              {pinned.map((s) => (
                                <li key={s.id}>
                                  <button
                                    type="button"
                                    className="spike-one-pinned-row"
                                    data-open={
                                      cells.some((cell) => cell.sessionId === s.id) ||
                                      undefined
                                    }
                                    draggable
                                    onDragStart={(event) => startDrag(event, s)}
                                    onClick={(event) =>
                                      mod(event) ? split(s.id) : open(s.id)
                                    }
                                    title={s.title}
                                  >
                                    <AgentTile agent={agentOf(s)} size={14} />
                                    <span className="spike-one-truncate">{s.title}</span>
                                    <StatusDot status={s.status} />
                                  </button>
                                </li>
                              ))}
                            </ul>
                          ) : null}
                        </li>
                      )
                    })}
                  </ul>
                </div>
              </section>
            )
          })}
        </div>

        <footer className="spike-one-identity">
          <span aria-hidden="true" className="desktop-mark" />
          <button
            type="button"
            className="spike-identity-button spike-one-identity-name spike-one-truncate"
            title={`Settings (${key("⌘,")})`}
            onClick={openSettings}
          >
            <b>nessa</b> <span>Studio</span>
          </button>
          <ThemeMenu theme={theme} onThemeChange={setTheme} />
        </footer>
      </nav>

      <div className="spike-one-sidebar-edge">
        <Edge
          label="Resize sidebar"
          onStart={() => (sidebarFrom.current = sidebarWidth)}
          onMove={(dx) => setSidebarWidth(clamp(sidebarFrom.current + dx, 200, 320))}
          onKey={(step) => setSidebarWidth((w) => clamp(w + step, 200, 320))}
        />
      </div>

      <SessionList
        open={listOpen}
        title={title}
        isChannel={view.kind === "channel"}
        sessions={listed}
        showChannel={view.kind === "smart"}
        query={query}
        onQuery={setQuery}
        searchRef={searchRef}
        openIds={cells.map((cell) => cell.sessionId)}
        focusedId={focusedSession?.id}
        onOpen={open}
        onSplit={(id) => split(id)}
        onNew={() => newSession()}
        onPin={togglePin}
        onArchive={archive}
        canSplit={cells.length < MAX_PANES}
      />

      <div className="spike-one-list-edge">
        <Edge
          label="Resize session list"
          onStart={() => (listFrom.current = listWidth)}
          onMove={(dx) => setListWidth(clamp(listFrom.current + dx, 260, 420))}
          onKey={(step) => setListWidth((w) => clamp(w + step, 260, 420))}
        />
      </div>

      <main className="spike-one-chat" aria-label="Conversations">
        <div
          className="spike-one-panes"
          ref={chatRef}
          data-multi={cells.length > 1 || undefined}
          data-moving={draggingPane !== null || undefined}
        >
          {placed.panes.map(({ cell, corner, style }) => {
            const session = byId.get(cell.sessionId)
            const multi = cells.length > 1
            return (
              <PaneSlot
                key={cell.key}
                paneKey={cell.key}
                style={style}
                corner={corner}
                session={session}
                showChannel={
                  !(listOpen && view.kind === "channel" && view.id === session?.channelId)
                }
                onSend={(text) => session && send(session.id, text)}
                onModelChange={(model) => session && patch(session.id, () => ({ model }))}
                focused={cell.key === focusedCell.key && multi}
                multi={multi}
                canSplit={cells.length < MAX_PANES}
                draggingPane={draggingPane}
                onPaneDrag={setDraggingPane}
                canPlace={canPlace}
                onFocus={() => setFocusedKey(cell.key)}
                onClose={() => closePane(cell.key)}
                onSplitNew={(side) => {
                  setFocusedKey(cell.key)
                  newSession(side, cell.key)
                }}
                onNudge={(direction) => nudge(cell.key, direction)}
                onDropSession={(id, zone) => {
                  const shown = cells.find((other) => other.sessionId === id)
                  if (shown) setFocusedKey(shown.key)
                  else if (zone === "center") {
                    setLayout((all) => showIn(all, cell.key, id))
                    setFocusedKey(cell.key)
                    markRead(id)
                  } else split(id, cell.key, zone)
                }}
                onDropPane={(from, zone) => movePane(from, cell.key, zone)}
                onApprove={(allow) => session && approve(session.id, allow)}
              />
            )
          })}
          {placed.edges.map((edge) => (
            <Edge
              key={edge.key}
              axis={edge.axis}
              style={edge.style}
              label={edge.axis === "x" ? "Resize columns" : "Resize panes"}
              onStart={() => (resizeFrom.current = layout)}
              onMove={(delta) =>
                edge.axis === "x"
                  ? resizeColumns(edge.c, delta)
                  : resizeRows(edge.c, edge.r, delta)
              }
              onKey={(step) => {
                resizeFrom.current = layout
                if (edge.axis === "x") resizeColumns(edge.c, step)
                else resizeRows(edge.c, edge.r, step)
              }}
            />
          ))}
        </div>
      </main>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/* Sidebar row                                                         */
/* ------------------------------------------------------------------ */

function SidebarRow({
  icon,
  label,
  active,
  unread,
  badge,
  badgeTone,
  running,
  onClick,
}: {
  icon: React.ReactNode
  label: string
  active: boolean
  unread?: boolean
  badge?: number
  badgeTone?: "needs"
  running?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className="spike-one-row"
      data-active={active || undefined}
      data-unread={unread || undefined}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      <span className="spike-one-row-icon" aria-hidden="true">
        {icon}
      </span>
      <span className="spike-one-truncate">{label}</span>
      {badge ? (
        <span className="spike-one-badge" data-tone={badgeTone}>
          {badge}
        </span>
      ) : running ? (
        <StatusDot status="running" />
      ) : null}
    </button>
  )
}

/* ------------------------------------------------------------------ */
/* Middle column                                                       */
/* ------------------------------------------------------------------ */

const groups: { status: Status; label: string }[] = [
  { status: "needs", label: "Needs you" },
  { status: "running", label: "Running" },
  { status: "idle", label: "Earlier" },
]

function SessionList({
  open,
  title,
  isChannel,
  sessions,
  showChannel,
  query,
  onQuery,
  searchRef,
  openIds,
  focusedId,
  onOpen,
  onSplit,
  onNew,
  onPin,
  onArchive,
  canSplit,
}: {
  open: boolean
  title: string
  isChannel: boolean
  sessions: Session[]
  showChannel: boolean
  query: string
  onQuery: (query: string) => void
  searchRef: React.RefObject<HTMLInputElement | null>
  openIds: string[]
  focusedId?: string
  onOpen: (id: string) => void
  onSplit: (id: string) => void
  onNew: () => void
  onPin: (id: string) => void
  onArchive: (id: string) => void
  canSplit: boolean
}) {
  const listRef = useRef<HTMLDivElement>(null)
  const ordered = groups.flatMap((group) =>
    sessions.filter((s) => s.status === group.status),
  )
  // One row takes Tab: the open one, or the first when none here is open.
  const tabStop = ordered.some((s) => s.id === focusedId) ? focusedId : ordered[0]?.id

  // Arrow keys walk the list and open as they go, like Mail's message list.
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return
    event.preventDefault()
    const at = ordered.findIndex((s) => s.id === focusedId)
    const next =
      ordered[
        Math.min(
          Math.max(at + (event.key === "ArrowDown" ? 1 : -1), 0),
          ordered.length - 1,
        )
      ]
    if (!next) return
    onOpen(next.id)
    listRef.current
      ?.querySelector<HTMLElement>(`[data-session="${next.id}"]`)
      ?.focus({ preventScroll: false })
  }

  return (
    <section
      className="spike-one-list"
      aria-label={title}
      inert={!open}
      aria-hidden={!open}
    >
      <div className="spike-one-list-inner">
        <header className="spike-one-list-head" data-tauri-drag-region>
          <div className="spike-one-list-title" data-tauri-drag-region>
            {isChannel ? <DesktopIcon name="channel" /> : null}
            <h2>{title}</h2>
          </div>
          <button
            type="button"
            className="spike-one-icon-button"
            aria-label={`New session (${key("⌘N")})`}
            title={`New session (${key("⌘N")})`}
            onClick={onNew}
          >
            <DesktopIcon name="newSession" />
          </button>
        </header>
        <label className="spike-one-search">
          <DesktopIcon name="search" />
          <input
            ref={searchRef}
            type="search"
            placeholder="Search sessions"
            value={query}
            onChange={(event) => onQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") onQuery("")
            }}
          />
          {query ? null : <kbd>{key("⌘F")}</kbd>}
        </label>

        <div
          className="spike-one-list-scroll"
          ref={listRef}
          role="listbox"
          aria-label="Sessions"
          onKeyDown={onKeyDown}
        >
          {sessions.length === 0 ? (
            <div className="spike-one-empty">
              <p>
                {query
                  ? "No sessions match."
                  : isChannel
                    ? "No sessions here yet."
                    : "Nothing here right now."}
              </p>
              {!query && isChannel ? (
                <button type="button" className="spike-one-button" onClick={onNew}>
                  New Session
                </button>
              ) : null}
            </div>
          ) : null}
          {groups.map((group) => {
            const items = sessions.filter((s) => s.status === group.status)
            if (items.length === 0) return null
            return (
              <div key={group.status} role="group" aria-label={group.label}>
                {/* A smart view holds one state, and its title already names it. */}
                {isChannel ? (
                  <h3 className="spike-one-group-label">
                    {group.label}
                    <span>{items.length}</span>
                  </h3>
                ) : null}
                {items.map((s) => {
                  const paneIndex = openIds.indexOf(s.id)
                  return (
                    <ContextMenu key={s.id}>
                      <ContextMenuTrigger asChild>
                        <div
                          role="option"
                          tabIndex={s.id === tabStop ? 0 : -1}
                          aria-selected={s.id === focusedId}
                          data-session={s.id}
                          className="spike-one-session"
                          data-selected={s.id === focusedId || undefined}
                          data-open={paneIndex >= 0 || undefined}
                          data-unread={s.unread || undefined}
                          draggable
                          onDragStart={(event: DragEvent<HTMLDivElement>) =>
                            startDrag(event, s)
                          }
                          onClick={(event: ReactMouseEvent) =>
                            mod(event) ? onSplit(s.id) : onOpen(s.id)
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault()
                              if (mod(event)) onSplit(s.id)
                              else onOpen(s.id)
                            }
                          }}
                        >
                          <AgentTile agent={agentOf(s)} size={22} />
                          <div className="spike-one-session-main">
                            <div className="spike-one-session-top">
                              {s.unread ? (
                                <span className="spike-one-unread" aria-label="Unread" />
                              ) : null}
                              <span className="spike-one-session-title spike-one-truncate">
                                {s.title}
                              </span>
                              <time>{s.time}</time>
                            </div>
                            {/* A short first message is the title too; it is said once. */}
                            {sameWords(s.title, s.preview) ? null : (
                              <p className="spike-one-session-preview">{s.preview}</p>
                            )}
                            {/* The group, or the view's title, already says the state. */}
                            {showChannel ? (
                              <div className="spike-one-session-meta spike-one-truncate">
                                #{channelName(s.channelId)}
                              </div>
                            ) : null}
                          </div>
                        </div>
                      </ContextMenuTrigger>
                      <ContextMenuContent className="desktop-popover spike-one-menu">
                        <ContextMenuItem onSelect={() => onOpen(s.id)}>
                          Open
                          <ContextMenuShortcut className="spike-one-shortcut">
                            ↩
                          </ContextMenuShortcut>
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() => onSplit(s.id)}
                          disabled={!canSplit && paneIndex < 0}
                        >
                          Open in Split
                          <ContextMenuShortcut className="spike-one-shortcut">
                            {key("⌘")}Click
                          </ContextMenuShortcut>
                        </ContextMenuItem>
                        <ContextMenuSeparator />
                        <ContextMenuItem onSelect={() => onPin(s.id)}>
                          {s.pinned ? "Unpin from Sidebar" : "Pin to Sidebar"}
                        </ContextMenuItem>
                        <ContextMenuItem onSelect={() => onArchive(s.id)}>
                          Archive
                        </ContextMenuItem>
                      </ContextMenuContent>
                    </ContextMenu>
                  )
                })}
              </div>
            )
          })}
        </div>
      </div>
    </section>
  )
}

/* ------------------------------------------------------------------ */
/* Chat panes                                                          */
/* ------------------------------------------------------------------ */

/** Where the first message was when it was sent from a new session's home. */
interface Arrival {
  composer: DOMRect
  text: DOMRect
}

const dropLabels: Record<"session" | "pane", Record<Zone, string>> = {
  session: {
    center: "Open here",
    left: "Split left",
    right: "Split right",
    top: "Split up",
    bottom: "Split down",
  },
  pane: {
    center: "Swap panes",
    left: "Move left",
    right: "Move right",
    top: "Move above",
    bottom: "Move below",
  },
}

function PaneSlot({
  paneKey,
  style,
  corner,
  session,
  showChannel,
  focused,
  multi,
  canSplit,
  draggingPane,
  onPaneDrag,
  canPlace,
  onSend,
  onModelChange,
  onFocus,
  onClose,
  onSplitNew,
  onNudge,
  onDropSession,
  onDropPane,
  onApprove,
}: {
  paneKey: number
  style: React.CSSProperties
  /** The top-left pane, which clears the window controls when the columns are hidden. */
  corner: boolean
  session?: Session
  /** Whether the heading names the channel: not when the list beside it already does. */
  showChannel: boolean
  focused: boolean
  multi: boolean
  canSplit: boolean
  draggingPane: number | null
  onPaneDrag: (key: number | null) => void
  canPlace: (side: Side, target: number, moving?: number) => boolean
  onSend: (text: string) => void
  /** The model picked in this pane's composer, kept as the session's own. */
  onModelChange: (model: ModelRef) => void
  onFocus: () => void
  onClose: () => void
  onSplitNew: (side: Side) => void
  onNudge: (direction: Direction) => void
  onDropSession: (id: string, zone: Zone) => void
  onDropPane: (from: number, zone: Zone) => void
  onApprove: (allow: boolean) => void
}) {
  const [zone, setZone] = useState<Zone | null>(null)
  const [dropKind, setDropKind] = useState<"session" | "pane">("session")
  const [headingVisible, setHeadingVisible] = useState(false)
  const [leaving, setLeaving] = useState(false)
  const [arrival, setArrival] = useState<Arrival | null>(null)
  const leaveTimer = useRef(0)
  useEffect(() => () => clearTimeout(leaveTimer.current), [])

  const fresh = !session || !!session.fresh
  const showHome = fresh || leaving

  // The drop target nearest the pointer's edge, or the middle to open in place.
  const zoneFor = (event: DragEvent<HTMLElement>, moving?: number): Zone => {
    const box = event.currentTarget.getBoundingClientRect()
    const x = (event.clientX - box.left) / box.width
    const y = (event.clientY - box.top) / box.height
    const near: [Side, number][] = [
      ["left", x],
      ["right", 1 - x],
      ["top", y],
      ["bottom", 1 - y],
    ]
    const [side, distance] = near.reduce((a, b) => (b[1] < a[1] ? b : a))
    if (distance > 0.26) return "center"
    if (moving === undefined && !canSplit) return "center"
    return canPlace(side, paneKey, moving) ? side : "center"
  }

  const homeRef = useRef<HTMLDivElement>(null)
  // The caret follows the first message into the conversation, with or
  // without the motion that carries it there.
  const handoff = useRef<string | null>(null)
  const sendFromHome = (text: string) => {
    if (!session) return
    handoff.current = session.id
    const composer = homeRef.current?.querySelector(".desktop-composer")
    const field = composer?.querySelector("textarea")
    if (composer && field && !reducedMotion()) {
      // Measured before the conversation replaces the home, so the composer
      // can travel from here to its place at the foot of the pane.
      setArrival({
        composer: composer.getBoundingClientRect(),
        text: field.getBoundingClientRect(),
      })
      setLeaving(true)
      clearTimeout(leaveTimer.current)
      leaveTimer.current = window.setTimeout(() => {
        setLeaving(false)
        setArrival(null)
      }, ARRIVAL_MS)
    }
    onSend(text)
  }

  const title = fresh ? "New session" : (session?.title ?? "")
  // A new session's home speaks for itself; a conversation's heading says the
  // title, so the bar shows it only once that heading has scrolled away.
  const titleShown = !fresh && !headingVisible
  const lifted = draggingPane === paneKey

  return (
    <article
      className="spike-one-pane"
      style={style}
      data-pane-key={paneKey}
      data-corner={corner || undefined}
      data-focused={focused || undefined}
      data-lifted={lifted || undefined}
      aria-label={title || "Empty pane"}
      onPointerDown={onFocus}
      onFocusCapture={onFocus}
      onDragOver={(event) => {
        const types = event.dataTransfer.types
        const pane = types.includes(PANE_TYPE)
        if (!pane && !types.includes(DRAG_TYPE)) return
        if (pane && (draggingPane === null || lifted)) return
        event.preventDefault()
        event.dataTransfer.dropEffect = pane ? "move" : "copy"
        setDropKind(pane ? "pane" : "session")
        const next = zoneFor(event, pane ? (draggingPane ?? undefined) : undefined)
        if (next !== zone) setZone(next)
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null))
          setZone(null)
      }}
      onDrop={(event) => {
        const at = zone ?? "center"
        setZone(null)
        if (event.dataTransfer.types.includes(PANE_TYPE)) {
          event.preventDefault()
          if (draggingPane !== null) onDropPane(draggingPane, at)
          onPaneDrag(null)
          return
        }
        const id = event.dataTransfer.getData(DRAG_TYPE)
        if (id) {
          event.preventDefault()
          onDropSession(id, at)
        }
      }}
    >
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <header
            className="spike-one-pane-head"
            // With one pane the header moves the window; with more it moves the pane.
            data-tauri-drag-region={multi ? undefined : true}
            draggable={multi || undefined}
            onDragStart={(event) => {
              if (!multi) return
              event.dataTransfer.setData(PANE_TYPE, String(paneKey))
              event.dataTransfer.effectAllowed = "move"
              setDragGhost(event, title)
              onPaneDrag(paneKey)
            }}
            onDragEnd={() => onPaneDrag(null)}
          >
            <div
              className="spike-one-pane-name"
              data-shown={titleShown || undefined}
              aria-hidden={!titleShown}
            >
              {session && !fresh ? (
                <AgentTile agent={agentOf(session)} size={16} />
              ) : null}
              <span className="spike-one-pane-title spike-one-truncate" title={title}>
                {title}
              </span>
              {session ? <StatusDot status={session.status} /> : null}
            </div>
            <span className="spike-one-spacer" data-tauri-drag-region />
            <div className="spike-one-pane-actions">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="spike-one-icon-button"
                    aria-label="Pane actions"
                    title="Pane actions"
                    draggable={false}
                  >
                    <DesktopIcon name="moreHorizontal" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  sideOffset={6}
                  className="desktop-popover spike-one-menu"
                >
                  <DropdownMenuItem
                    disabled={!canSplit}
                    onSelect={() => onSplitNew("right")}
                  >
                    Split Right
                    <DropdownMenuShortcut className="spike-one-shortcut">
                      {key("⌘\\")}
                    </DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    disabled={!canSplit}
                    onSelect={() => onSplitNew("bottom")}
                  >
                    Split Down
                    <DropdownMenuShortcut className="spike-one-shortcut">
                      {key("⇧⌘\\")}
                    </DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem disabled={!multi} onSelect={onClose}>
                    Close Pane
                    <DropdownMenuShortcut className="spike-one-shortcut">
                      {key("⌘W")}
                    </DropdownMenuShortcut>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
              {multi ? (
                <button
                  type="button"
                  className="spike-one-icon-button"
                  aria-label={`Close Pane (${key("⌘W")})`}
                  title={`Close Pane (${key("⌘W")})`}
                  draggable={false}
                  onClick={(event) => {
                    event.stopPropagation()
                    onClose()
                  }}
                >
                  <DesktopIcon name="close" />
                </button>
              ) : null}
            </div>
          </header>
        </ContextMenuTrigger>
        <ContextMenuContent className="desktop-popover spike-one-menu">
          <ContextMenuItem disabled={!canSplit} onSelect={() => onSplitNew("right")}>
            Split Right
            <ContextMenuShortcut className="spike-one-shortcut">
              {key("⌘\\")}
            </ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem disabled={!canSplit} onSelect={() => onSplitNew("bottom")}>
            Split Down
            <ContextMenuShortcut className="spike-one-shortcut">
              {key("⇧⌘\\")}
            </ContextMenuShortcut>
          </ContextMenuItem>
          {multi ? (
            <>
              <ContextMenuSeparator />
              {(
                [
                  ["left", "Move Left", "←"],
                  ["right", "Move Right", "→"],
                  ["up", "Move Up", "↑"],
                  ["down", "Move Down", "↓"],
                ] as const
              ).map(([direction, label, arrow]) => (
                <ContextMenuItem key={direction} onSelect={() => onNudge(direction)}>
                  {label}
                  <ContextMenuShortcut className="spike-one-shortcut">
                    {isMac ? `⌃⌥${arrow}` : `Ctrl+Alt+${arrow}`}
                  </ContextMenuShortcut>
                </ContextMenuItem>
              ))}
              <ContextMenuSeparator />
              <ContextMenuItem onSelect={onClose}>
                Close Pane
                <ContextMenuShortcut className="spike-one-shortcut">
                  {key("⌘W")}
                </ContextMenuShortcut>
              </ContextMenuItem>
            </>
          ) : null}
        </ContextMenuContent>
      </ContextMenu>

      <div className="spike-one-pane-body">
        {showHome ? (
          <div
            key={`home-${session?.id}`}
            ref={homeRef}
            className="spike-one-home"
            data-leaving={leaving || undefined}
          >
            <PaneHome
              initialModel={session?.model}
              onSend={fresh && session ? sendFromHome : undefined}
              onModelChange={onModelChange}
            />
          </div>
        ) : null}
        {session && !fresh ? (
          <Conversation
            key={session.id}
            session={session}
            arrival={arrival}
            takeFocus={() => {
              const mine = handoff.current === session.id
              handoff.current = null
              return mine
            }}
            showChannel={showChannel}
            onSend={onSend}
            onModelChange={onModelChange}
            onHeading={setHeadingVisible}
            onApprove={onApprove}
          />
        ) : null}
      </div>

      {zone ? (
        <div className="spike-one-drop" data-zone={zone} aria-hidden="true">
          <span>
            {zone === "center" && dropKind === "session" && !canSplit
              ? "Replace this pane"
              : dropLabels[dropKind][zone]}
          </span>
        </div>
      ) : null}
    </article>
  )
}

/**
 * A new session's home: the window's own scene, greeting and composer, as
 * `Home` lays them out, with the composer wired to the session. `Home` takes
 * no composer props, so its three parts are composed here.
 */
function PaneHome({
  initialModel,
  onSend,
  onModelChange,
}: {
  initialModel?: ModelRef
  onSend?: (text: string) => void
  onModelChange: (model: ModelRef) => void
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
            initialModel={initialModel}
            onModelChange={onModelChange}
            onSend={onSend}
          />
        </div>
      </div>
    </div>
  )
}

function Conversation({
  session,
  arrival,
  takeFocus,
  showChannel,
  onSend,
  onModelChange,
  onHeading,
  onApprove,
}: {
  session: Session
  /** Set when this conversation replaces a home that just sent its first message. */
  arrival: Arrival | null
  /** Asked once, on mount: whether to take the caret, the person having typed in the home it replaces. */
  takeFocus: () => boolean
  showChannel: boolean
  onSend: (text: string) => void
  onModelChange: (model: ModelRef) => void
  /** Reports whether the heading's title is in view, so the pane header needn't repeat it. */
  onHeading: (visible: boolean) => void
  onApprove: (allow: boolean) => void
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const headingRef = useRef<HTMLDivElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const dockRef = useRef<HTMLDivElement>(null)
  // Whether this conversation arrived from a home is fixed at mount: the
  // arrival clearing later must not replay the ordinary opening.
  const [arrived] = useState(() => arrival !== null)
  const arrivalAtMount = useRef(arrival)
  // Turns that arrive while this conversation is open rise into place.
  const [settled] = useState(() => (arrival ? 1 : session.turns.length))
  const [, setPage] = useState(false)
  const report = useRef(onHeading)
  report.current = onHeading
  const agent = agentOf(session)

  // Opening lands on the latest turn, before paint; the header shows the
  // title only once the heading's own title has scrolled out of view.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const title = titleRef.current
    if (!scroller || !title) return
    if (!arrivalAtMount.current) scroller.scrollTop = scroller.scrollHeight
    const top = scroller.getBoundingClientRect().top
    report.current(title.getBoundingClientRect().bottom > top + 4)
    const observer = new IntersectionObserver(
      ([entry]) => report.current(entry.isIntersecting),
      { root: scroller, rootMargin: "-4px 0px 0px 0px" },
    )
    observer.observe(title)
    return () => observer.disconnect()
  }, [])

  const focusAtMount = useRef(takeFocus)
  useLayoutEffect(() => {
    if (focusAtMount.current())
      dockRef.current?.querySelector("textarea")?.focus({ preventScroll: true })
  }, [])

  // The first message's arrival: the composer glides from the home down to
  // the foot of the pane, the message rises from where it was typed into its
  // bubble, and the heading settles in above it. Transform and opacity only.
  useLayoutEffect(() => {
    const from = arrivalAtMount.current
    const composer = dockRef.current?.querySelector<HTMLElement>(".desktop-composer")
    const bubble = scrollRef.current?.querySelector<HTMLElement>(".spike-one-user p")
    const heading = headingRef.current
    if (!from || !composer || !bubble || !heading) return
    const to = composer.getBoundingClientRect()
    const scale = from.composer.width / to.width
    const dx = from.composer.left + from.composer.width / 2 - (to.left + to.width / 2)
    const dy = from.composer.top + from.composer.height / 2 - (to.top + to.height / 2)
    const glide = composer.animate(
      [
        { transform: `translate(${dx}px, ${dy}px) scale(${scale})` },
        { transform: "none" },
      ],
      { duration: 420, easing: SPRING },
    )
    // The message leaves the field as the composer travels; its placeholder
    // returns only once the field has nearly settled.
    const field = composer.querySelector("textarea")
    const clear = field?.animate(
      [{ opacity: 0 }, { opacity: 0, offset: 0.6 }, { opacity: 1 }],
      { duration: 420, easing: "ease-out" },
    )
    const b = bubble.getBoundingClientRect()
    const rise = bubble.animate(
      [
        {
          transform: `translate(${from.text.left - b.left - 14}px, ${from.text.top - b.top - 10}px)`,
          backgroundColor: "transparent",
        },
        { offset: 0.4, backgroundColor: "transparent" },
        { transform: "none" },
      ],
      { duration: 400, easing: SPRING },
    )
    const settle = heading.animate(
      [
        { opacity: 0, transform: "translateY(8px)" },
        { opacity: 1, transform: "none" },
      ],
      { duration: 280, delay: 80, easing: EASE, fill: "backwards" },
    )
    // Cancelled on cleanup, so a second mount measures the resting layout
    // rather than a frame of this animation.
    return () => [glide, rise, settle, clear].forEach((animation) => animation?.cancel())
  }, [])

  // A reply that's streaming keeps the latest line in view, unless the
  // person has scrolled up to read. While the arrival plays, the transcript
  // holds still under it.
  const last = session.turns[session.turns.length - 1]
  const lastLength = last && "text" in last ? last.text.length : 0
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    const fromBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight
    if (fromBottom <= 0 || fromBottom >= 160) return
    scroller.scrollTo({
      top: scroller.scrollHeight,
      behavior: reducedMotion() ? "auto" : "smooth",
    })
  }, [session.turns.length, lastLength, session.status])

  const working = session.status === "running" && !session.streaming

  return (
    <div className="spike-one-conversation" data-arriving={arrived || undefined}>
      <div className="spike-one-transcript" ref={scrollRef}>
        <div className="spike-one-transcript-inner">
          <div className="spike-one-heading" ref={headingRef}>
            <h2 ref={titleRef}>{session.title}</h2>
            <p>
              {showChannel ? `#${channelName(session.channelId)} · ` : null}
              {startedLabel(session, !showChannel)}
            </p>
          </div>
          {session.turns.map((turn, i) =>
            turn.kind === "agent" && !turn.text ? null : (
              <TurnView
                key={i}
                turn={turn}
                agent={agent}
                // The agent's mark leads a reply once, not every paragraph of it.
                marked={session.turns[i - 1]?.kind === "user"}
                isNew={i >= settled}
                onApprove={onApprove}
              />
            ),
          )}
          {working ? (
            <div className="spike-one-working" role="status">
              <AgentTile agent={agent} size={20} />
              <span>
                {agentNames[agent]} is{" "}
                {session.turns.length <= 1 ? "thinking" : "working"}
              </span>
            </div>
          ) : null}
        </div>
      </div>
      <div className="spike-one-composer" ref={dockRef}>
        {/* In a conversation the field is a reply, and says so in fewer words than the home's question. */}
        <Composer
          page={false}
          onPageChange={setPage}
          initialModel={session.model}
          onModelChange={onModelChange}
          onSend={onSend}
          placeholder="Reply…"
        />
      </div>
    </div>
  )
}

function TurnView({
  turn,
  agent,
  marked,
  isNew,
  onApprove,
}: {
  turn: Turn
  agent: AgentId
  marked: boolean
  /** Arrived while the conversation was open, so it rises into place. */
  isNew: boolean
  onApprove: (allow: boolean) => void
}) {
  const fresh = isNew || undefined
  if (turn.kind === "user") {
    return (
      <div className="spike-one-user" data-new={fresh}>
        <p>{turn.text}</p>
      </div>
    )
  }
  if (turn.kind === "agent") {
    return (
      <div className="spike-one-agent-turn" data-new={fresh}>
        {marked ? <AgentTile agent={agent} size={20} /> : <span aria-hidden="true" />}
        <p>{turn.text}</p>
      </div>
    )
  }
  if (turn.kind === "tool") {
    const icon = turn.tool === "run" ? "terminal" : turn.tool === "edit" ? "edit" : "file"
    return (
      <div className="spike-one-tool" data-new={fresh}>
        <DesktopIcon name={icon} />
        <span>{turn.label}</span>
        {turn.detail ? <code>{turn.detail}</code> : null}
      </div>
    )
  }
  return (
    <div
      className="spike-one-approval"
      role="group"
      aria-label="Approval needed"
      data-new={fresh}
    >
      <div className="spike-one-approval-head">
        <DesktopIcon name="needsYou" />
        <span>{agentNames[agent]} wants to run a command</span>
      </div>
      <code>{turn.command}</code>
      <p>{turn.reason}</p>
      <div className="spike-one-approval-actions">
        <button
          type="button"
          className="spike-one-button"
          onClick={() => onApprove(false)}
        >
          Deny
        </button>
        <button
          type="button"
          className="spike-one-button"
          data-primary
          onClick={() => onApprove(true)}
        >
          Allow once
        </button>
      </div>
    </div>
  )
}
