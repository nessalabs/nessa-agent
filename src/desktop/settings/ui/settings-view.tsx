import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import type { HostKind } from "../../../host/features"
import { useThemePreference } from "../../adapters/theme-preference"
import {
  firstTabOf,
  searchSettings,
  settingsCategories,
  settingsCategory,
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

const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)
const shortcut = (keys: string) => (isMac ? keys : keys.replace("⌘", "Ctrl+"))

/** Each category's icon in the sidebar, for every category the catalogue has. */
const categoryIcons: Record<SettingsCategoryId, DesktopIconRole> = {
  general: "preferences",
  appearance: "appearance",
  workspace: "workspace",
  models: "model",
  connections: "connections",
  privacy: "privacy",
  about: "about",
}

interface SettingsHostProps {
  hostKind: HostKind
  browserSurface: boolean
}

/** Mounts once over the app; renders nothing until opened. */
export function SettingsHost(props: SettingsHostProps) {
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
  return open ? <SettingsView {...props} onClose={() => setOpen(false)} /> : null
}

function SettingsView({
  hostKind,
  browserSurface,
  onClose,
}: SettingsHostProps & { onClose: () => void }) {
  const [theme] = useThemePreference()
  const [sidebarOpen, setSidebarOpen] = useState(true)
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
  const tab = chosenTabs[category] ?? firstTabOf(category)
  const results = useMemo(() => searchSettings(query), [query])
  const Page = settingsTabPages[tab]

  useEffect(() => {
    rootRef.current?.focus()
  }, [])

  // ⌘B toggles this sidebar, and only this one: caught before the app's own
  // listeners, so the sidebar under Settings stays as it was. Settings is
  // modal, so the window's other shortcuts wait too — ⌘W must not close a
  // pane nobody can see. Only their listeners are stopped: text editing in
  // Settings' own fields is the browser's default, and still happens.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const mod = isMac ? event.metaKey : event.ctrlKey
      if (!mod && !event.ctrlKey) return
      event.stopPropagation()
      if (!mod || event.altKey || event.shiftKey || event.code !== "KeyB") return
      event.preventDefault()
      setSidebarOpen((value) => !value)
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [])

  // Focus left in a sidebar that slides away would be lost; it moves to the toggle.
  useEffect(() => {
    if (!sidebarOpen && sidebarRef.current?.contains(document.activeElement)) {
      toggleRef.current?.focus()
    }
  }, [sidebarOpen])

  // A new tab starts at its top; a jump to a setting brings that setting into view.
  useLayoutEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [tab])
  useEffect(() => {
    if (!found) return
    const row = scrollRef.current?.querySelector(`[data-setting="${found}"]`)
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    row?.scrollIntoView({ block: "nearest", behavior: reduce ? "auto" : "smooth" })
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

  const sidebarLabel = `${sidebarOpen ? "Hide" : "Show"} Sidebar (${shortcut("⌘B")})`

  return (
    <div
      ref={rootRef}
      className="settings"
      data-host={hostKind}
      data-surface={browserSurface ? "browser" : "window"}
      data-desktop-theme={theme}
      data-sidebar={sidebarOpen ? "open" : "closed"}
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
          title={sidebarLabel}
          aria-expanded={sidebarOpen}
          aria-controls="settings-sidebar"
          onClick={() => setSidebarOpen((value) => !value)}
        >
          <DesktopIcon name="sidebar" />
        </button>
        <button
          type="button"
          className="settings-titlebar-button settings-titlebar-back"
          aria-label="Back to nessa Agent"
          title="Back to nessa Agent"
          tabIndex={sidebarOpen ? -1 : 0}
          aria-hidden={sidebarOpen || undefined}
          onClick={onClose}
        >
          <DesktopIcon name="chevronLeft" />
        </button>
      </div>

      <div
        ref={sidebarRef}
        id="settings-sidebar"
        className="settings-sidebar"
        inert={!sidebarOpen}
        aria-hidden={!sidebarOpen || undefined}
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
          <button type="button" className="settings-identity" onClick={onClose}>
            <DesktopIcon name="chevronLeft" className="settings-identity-back" />
            <span className="settings-identity-name">
              <strong>nessa</strong>
              <span>Agent</span>
            </span>
          </button>
        </nav>
      </div>

      <main className="settings-content" aria-labelledby="settings-heading">
        <header className="settings-head">
          <div className="settings-column">
            <h1 id="settings-heading" key={category}>
              {settingsCategory(category).label}
            </h1>
            {tabs.length > 1 ? (
              <SettingsTabStrip
                key={`tabs-${category}`}
                tabs={tabs}
                selected={tab}
                onSelect={showTab}
              />
            ) : null}
          </div>
        </header>
        <div ref={scrollRef} className="settings-scroll">
          <div
            key={tab}
            className="settings-column settings-panel"
            id="settings-panel"
            role={tabs.length > 1 ? "tabpanel" : undefined}
            aria-labelledby={tabs.length > 1 ? tabButtonId(tab) : undefined}
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
