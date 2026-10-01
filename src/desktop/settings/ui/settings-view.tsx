import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import type { HostKind } from "../../../host/features"
import { holdStill, slideFrom } from "../../adapters/hold-still"
import { isMac } from "../../adapters/platform"
import { chordLabel, commandKey, matchesChord, type Chord } from "../../model/keyboard"
import { reducedMotion } from "../../adapters/motion-preference"
import { useThemePreference } from "../../adapters/theme-preference"
import { useEdgePeek } from "../../adapters/use-edge-peek"
import { useWindowWidth } from "../../adapters/window-width"
import { chosen, drawn, fitted, type SideColumn } from "../../model/side-column"
import { ColumnHeader } from "../../ui/column-header"
import { EdgePeekStrip } from "../../ui/edge-peek-strip"
import { HistoryButtons } from "../../ui/history-buttons"
import { focusInFront } from "../../workspace"
import { settingsSidebar, settingsSidebarFits } from "../model/settings-sidebar"
import {
  firstTabOf,
  searchSettings,
  settingsCategories,
  settingsCategory,
  showsTabs,
  tabsOf,
  type SettingId,
  type SettingsCategoryId,
  type SettingsMatch,
  type SettingsTab,
  type SettingsTabId,
} from "../model/settings-catalogue"
import { DesktopIcon, type DesktopIconRole } from "../../ui/icons"
import { FoundSetting } from "./settings-controls"
import { settingsTabPages } from "./settings-tabs"
import "./settings.css"
import { tooltip } from "../../ui/tooltip"

/**
 * Prototype: Settings as its own surface over the window. A short sidebar of
 * categories and, beside it, the chosen category's page: its title, a strip
 * of tabs, and the tab's settings in grouped rows, the way System Settings
 * lays them out. What exists is catalogued in `model/settings-catalogue.ts`;
 * the pages are in `settings-tabs.tsx`.
 *
 * Opened from "nessa Studio" at the foot of the app's sidebar (or ⌘,); in
 * Settings that same place reads "nessa Agent" with a back chevron, and
 * returns to the app. The sidebar toggle sits where the app's does and ⌘B
 * works the same; with the sidebar hidden, a back chevron beside the toggle
 * is the way out. Esc does not close Settings.
 */
const openEvent = "nessa:open-settings"

/** Opens Settings from anywhere in the window. */
export function openSettings() {
  window.dispatchEvent(new Event(openEvent))
}

/** ⌘B shows and hides Settings' sidebar, as it does the window's. */
const sidebarChord: Chord = { code: "KeyB", command: true }

/** Each category's icon in the sidebar, for every category the catalogue has. */
const categoryIcons: Record<SettingsCategoryId, DesktopIconRole> = {
  general: "preferences",
  appearance: "appearance",
  workspace: "workspace",
  models: "model",
  connections: "connections",
  privacy: "privacy",
  advanced: "advanced",
  about: "about",
}

interface SettingsHostProps {
  hostKind: HostKind
  browserSurface: boolean
}

/**
 * Whether Settings is open, and a way to close it: opened by `openSettings`
 * from anywhere, or ⌘,. The window holds this, so what lies under Settings
 * can be made inert while it is open.
 */
export function useSettingsOpening(): { open: boolean; close: () => void } {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const show = () => setOpen(true)
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ",") {
        event.preventDefault()
        setOpen(true)
      }
    }
    window.addEventListener(openEvent, show)
    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener(openEvent, show)
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [])
  const close = useCallback(() => setOpen(false), [])
  return { open, close }
}

/**
 * Settings over the window. Modal: the window under it is inert while it is
 * open (the window does that, from `useSettingsOpening`), and focus goes back
 * to what opened it when it closes — or, gone meanwhile, to the focused
 * pane's composer.
 */
export function SettingsHost({
  open,
  onClose,
  ...props
}: SettingsHostProps & { open: boolean; onClose: () => void }) {
  return open ? <SettingsView {...props} onClose={onClose} /> : null
}

function SettingsView({
  hostKind,
  browserSurface,
  onClose,
}: SettingsHostProps & { onClose: () => void }) {
  const [theme] = useThemePreference()
  // Shown or hidden by the person; folded by the window when a page would be
  // squeezed beside it, and back once there is room (`side-column.ts`).
  const windowWidth = useWindowWidth()
  // Refitted as the window's width changes, in the render that sees it — not
  // in a later effect, which a resize observer's synchronous render would
  // flush out of turn.
  const [sidebar, setSidebar] = useState(() => ({
    column: fitted({ open: true, folded: false }, settingsSidebarFits(windowWidth)),
    width: windowWidth,
  }))
  if (sidebar.width !== windowWidth)
    setSidebar({
      column: fitted(sidebar.column, settingsSidebarFits(windowWidth)),
      width: windowWidth,
    })
  const sidebarColumn =
    sidebar.width === windowWidth
      ? sidebar.column
      : fitted(sidebar.column, settingsSidebarFits(windowWidth))
  const setSidebarColumn = useCallback(
    (change: (column: SideColumn) => SideColumn) =>
      setSidebar((held) => ({ ...held, column: change(held.column) })),
    [],
  )
  const sidebarOpen = drawn(sidebarColumn)
  const toggleSidebar = useCallback(
    () => setSidebarColumn((column) => chosen(column)),
    [setSidebarColumn],
  )
  // Folded, the sidebar can be revealed from the window's edge, as the app's can.
  const peek = useEdgePeek(!sidebarOpen, sidebarOpen)
  const revealed = peek.shown && !peek.handingOff
  const [category, setCategory] = useState<SettingsCategoryId>("appearance")
  // The tab last shown in each category, kept while Settings is open.
  const [chosenTabs, setChosenTabs] = useState<
    Partial<Record<SettingsCategoryId, SettingsTabId>>
  >({})
  const [query, setQuery] = useState("")
  // The setting a search just jumped to, shown briefly on its row.
  const [found, setFound] = useState<SettingId | null>(null)
  // The search result last taken, marked in the results while they stay up.
  const [picked, setPicked] = useState<string | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const sidebarRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const resultsRef = useRef<HTMLUListElement>(null)

  const tabs = tabsOf(category)
  const tabsShown = showsTabs(category)
  const tab = chosenTabs[category] ?? firstTabOf(category)
  const results = useMemo(() => searchSettings(query), [query])
  const Page = settingsTabPages[tab]

  // Focus comes in on opening and goes back on closing: to what opened
  // Settings, or — gone meanwhile — to what is in front (`focusInFront`).
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null
    rootRef.current?.focus()
    return () => {
      if (opener && opener !== document.body && opener.isConnected)
        opener.focus({ preventScroll: true })
      else focusInFront()
    }
  }, [])

  // ⌘B toggles this sidebar, and only this one: caught before the app's own
  // listeners, so the sidebar under Settings stays as it was. Settings is
  // modal, so the window's other shortcuts wait too — ⌘W must not close a
  // pane nobody can see. Only their listeners are stopped: text editing in
  // Settings' own fields is the browser's default, and still happens.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!commandKey(event, isMac) && !event.ctrlKey) return
      event.stopPropagation()
      if (!matchesChord(event, sidebarChord, isMac)) return
      event.preventDefault()
      toggleSidebar()
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [toggleSidebar])

  // The sidebar's room changes at once, and the page slides into its new
  // place by transform, as the workspace's columns do; an inline title holds
  // still at its own place meanwhile (`holdStill`), clear of the window's
  // controls, rather than ride the slide from under them.
  const sidebarWas = useRef(sidebarOpen)
  useLayoutEffect(() => {
    const was = sidebarWas.current
    sidebarWas.current = sidebarOpen
    if (was === sidebarOpen) return
    const content = rootRef.current?.querySelector<HTMLElement>(".settings-content")
    if (!content) return
    const width = settingsSidebar.width
    if (!slideFrom(content, (was ? width : 0) - (sidebarOpen ? width : 0))) return
    const title = content.querySelector<HTMLElement>(
      ":scope > .desktop-column-bar > .desktop-column-title:not(.desktop-column-sizer)",
    )
    if (title) holdStill(title)
  }, [sidebarOpen])

  // Focus left in a sidebar that slides away — folded, or its reveal over —
  // would be lost; it moves to the toggle.
  useEffect(() => {
    const active = document.activeElement
    // Inert, it may have let go of focus already, to the page.
    const lost = active === document.body || sidebarRef.current?.contains(active)
    if (!sidebarOpen && !revealed && lost) toggleRef.current?.focus()
  }, [sidebarOpen, revealed])

  // A new tab starts at its top; a jump to a setting brings that setting into view.
  useLayoutEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [tab])
  useEffect(() => {
    if (!found) return
    const row = scrollRef.current?.querySelector(`[data-setting="${found}"]`)
    row?.scrollIntoView({
      block: "nearest",
      behavior: reducedMotion() ? "auto" : "smooth",
    })
    const timer = window.setTimeout(() => setFound(null), 1600)
    return () => window.clearTimeout(timer)
  }, [found])

  const showTab = (next: SettingsTab) => {
    setChosenTabs((all) => ({ ...all, [next.category]: next.id }))
  }

  const go = (match: SettingsMatch) => {
    setCategory(match.category)
    setChosenTabs((all) => ({ ...all, [match.category]: match.tab }))
    setFound(match.setting ?? null)
    setPicked(resultKey(match))
  }

  const moveInResults = (event: ReactKeyboardEvent, index: number) => {
    const buttons = resultsRef.current?.querySelectorAll("button")
    if (!buttons) return
    if (event.key === "ArrowDown") {
      event.preventDefault()
      buttons[Math.min(index + 1, buttons.length - 1)]?.focus()
    } else if (event.key === "ArrowUp") {
      event.preventDefault()
      if (index <= 0) searchRef.current?.focus()
      else buttons[index - 1]?.focus()
    }
  }

  const sidebarAction = `${sidebarOpen ? "Hide" : "Show"} Sidebar`
  const sidebarLabel = `${sidebarAction} (${chordLabel(sidebarChord, isMac)})`

  return (
    <div
      ref={rootRef}
      className="settings"
      data-host={hostKind}
      data-surface={browserSurface ? "browser" : "window"}
      data-desktop-theme={theme}
      data-sidebar={sidebarOpen ? "open" : "closed"}
      data-peek={revealed || undefined}
      style={{ "--settings-sidebar-w": `${settingsSidebar.width}px` } as CSSProperties}
      role="dialog"
      aria-modal="true"
      aria-label="Settings"
      tabIndex={-1}
    >
      <div className="desktop-ambient" aria-hidden="true">
        <span className="desktop-grain" />
      </div>

      {/* The drag strip and its stationary controls, where the app keeps its own. */}
      <div className="settings-titlebar" data-tauri-drag-region>
        <button
          ref={toggleRef}
          type="button"
          className="settings-titlebar-button"
          aria-label={sidebarLabel}
          {...tooltip(sidebarAction, { shortcut: chordLabel(sidebarChord, isMac) })}
          aria-expanded={sidebarOpen}
          aria-controls="settings-sidebar"
          onClick={toggleSidebar}
        >
          <DesktopIcon name="sidebar" />
        </button>
        <HistoryButtons className="settings-titlebar-button" />
        <button
          type="button"
          className="settings-titlebar-button settings-titlebar-back"
          aria-label="Back to nessa Agent"
          {...tooltip("Back to nessa Agent")}
          tabIndex={sidebarOpen ? -1 : 0}
          aria-hidden={sidebarOpen || undefined}
          onClick={onClose}
        >
          <DesktopIcon name="chevronLeft" />
        </button>
      </div>

      {sidebarOpen ? null : (
        <EdgePeekStrip
          peek={peek}
          onDragOut={() => setSidebarColumn((column) => chosen(column, true))}
        />
      )}
      <div
        ref={sidebarRef}
        id="settings-sidebar"
        className="settings-sidebar"
        inert={!sidebarOpen && !revealed}
        aria-hidden={(!sidebarOpen && !revealed) || undefined}
        {...(sidebarOpen ? {} : peek.holders)}
      >
        <nav className="settings-glass" aria-label="Settings categories">
          <label className="settings-search">
            <DesktopIcon name="search" />
            <input
              ref={searchRef}
              type="search"
              placeholder="Search"
              aria-label="Search settings"
              aria-controls="settings-results"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && results[0]) {
                  go(results[0])
                } else if (event.key === "ArrowDown") {
                  event.preventDefault()
                  resultsRef.current?.querySelector("button")?.focus()
                } else if (event.key === "Escape" && query) {
                  // Clears the search; Esc never closes Settings.
                  event.stopPropagation()
                  setQuery("")
                }
              }}
            />
            {query ? (
              <button
                type="button"
                className="settings-search-clear"
                aria-label="Clear search"
                onClick={() => {
                  setQuery("")
                  searchRef.current?.focus()
                }}
              >
                <DesktopIcon name="close" />
              </button>
            ) : null}
          </label>

          {query.trim() ? (
            <ul
              ref={resultsRef}
              id="settings-results"
              className="settings-nav"
              aria-label="Search results"
            >
              {results.map((match, index) => (
                <li key={resultKey(match)}>
                  <button
                    type="button"
                    className="settings-nav-item settings-result"
                    aria-current={resultKey(match) === picked ? "true" : undefined}
                    onClick={() => go(match)}
                    onKeyDown={(event) => moveInResults(event, index)}
                  >
                    <DesktopIcon
                      name={categoryIcons[match.category]}
                      className="settings-nav-icon"
                    />
                    <span className="settings-result-text">
                      <span>{match.label}</span>
                      {match.trail ? <small>{match.trail}</small> : null}
                    </span>
                  </button>
                </li>
              ))}
              {results.length === 0 ? (
                <li className="settings-nav-empty">No results for “{query.trim()}”</li>
              ) : null}
            </ul>
          ) : (
            <ul className="settings-nav">
              {settingsCategories.map((item) => (
                <li key={item.id}>
                  <button
                    type="button"
                    className="settings-nav-item"
                    aria-current={item.id === category ? "page" : undefined}
                    onClick={() => setCategory(item.id)}
                  >
                    <DesktopIcon
                      name={categoryIcons[item.id]}
                      className="settings-nav-icon"
                    />
                    {item.label}
                  </button>
                </li>
              ))}
            </ul>
          )}

          {/* Where "nessa Studio" opened Settings, the way back to the app. */}
          <button
            type="button"
            className="settings-identity"
            aria-label="Back to nessa Agent"
            onClick={onClose}
          >
            <DesktopIcon name="chevronLeft" className="settings-identity-back" />
            <span className="settings-identity-name">
              <strong>nessa</strong>
              <span>Agent</span>
            </span>
          </button>
        </nav>
      </div>

      <main className="settings-content" aria-labelledby="settings-heading">
        <ColumnHeader
          key={category}
          title={settingsCategory(category).label}
          heading="h1"
          headingId="settings-heading"
        >
          {tabsShown ? (
            <div className="settings-column settings-head-tabs">
              <SettingsTabStrip tabs={tabs} selected={tab} onSelect={showTab} />
            </div>
          ) : null}
        </ColumnHeader>
        <div ref={scrollRef} className="settings-scroll">
          <div
            key={tab}
            className="settings-column settings-panel"
            id="settings-panel"
            role={tabsShown ? "tabpanel" : undefined}
            aria-labelledby={tabsShown ? tabButtonId(tab) : undefined}
          >
            <FoundSetting.Provider value={found}>
              <Page />
            </FoundSetting.Provider>
          </div>
        </div>
      </main>
    </div>
  )
}

const tabButtonId = (tab: SettingsTabId) => `settings-tab-${tab}`
const resultKey = (match: SettingsMatch) => `${match.tab}:${match.setting ?? match.label}`

/**
 * The tabs across the top of a category's page. One tab stop: the arrow keys,
 * Home, and End move between tabs and show each as it is reached. A soft
 * pill slides under the selected tab; it is placed by measurement, written
 * straight to the strip's style so moving it is not a render.
 */
function SettingsTabStrip({
  tabs,
  selected,
  onSelect,
}: {
  tabs: readonly SettingsTab[]
  selected: SettingsTabId
  onSelect: (tab: SettingsTab) => void
}) {
  const listRef = useRef<HTMLDivElement>(null)

  useLayoutEffect(() => {
    const list = listRef.current
    if (!list) return
    const place = () => {
      const current = list.querySelector<HTMLElement>('[aria-selected="true"]')
      if (!current) return
      list.style.setProperty("--settings-tab-x", `${current.offsetLeft}px`)
      list.style.setProperty("--settings-tab-w", `${current.offsetWidth}px`)
    }
    place()
    // Only after the first placement does the pill glide, so it never slides in from nowhere.
    const frame = requestAnimationFrame(() => list.setAttribute("data-placed", ""))
    const observer = new ResizeObserver(place)
    observer.observe(list)
    return () => {
      cancelAnimationFrame(frame)
      observer.disconnect()
    }
  }, [selected])

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const index = tabs.findIndex((tab) => tab.id === selected)
    const next =
      event.key === "ArrowRight"
        ? (index + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (index - 1 + tabs.length) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : -1
    const target = tabs[next]
    if (!target) return
    event.preventDefault()
    onSelect(target)
    listRef.current?.querySelector<HTMLElement>(`#${tabButtonId(target.id)}`)?.focus()
  }

  return (
    <div
      ref={listRef}
      className="settings-tabs"
      role="tablist"
      aria-label="Sections"
      onKeyDown={onKeyDown}
    >
      {tabs.map((item) => (
        <button
          key={item.id}
          id={tabButtonId(item.id)}
          type="button"
          role="tab"
          className="settings-tab"
          aria-selected={item.id === selected}
          aria-controls="settings-panel"
          tabIndex={item.id === selected ? 0 : -1}
          onClick={() => onSelect(item)}
        >
          {item.label}
        </button>
      ))}
    </div>
  )
}
