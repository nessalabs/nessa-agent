/**
 * Settings' markup. It takes what `useSettingsSurface` decided and does not
 * read the workspace store or dispatch.
 */
import {
  useLayoutEffect,
  useRef,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react"
import { IdentityButton, IdentityRow } from "../../ui/identity"
import { EdgePeekStrip } from "../../ui/edge-peek-strip"
import { HistoryButtons } from "../../ui/history-buttons"
import { settingsSidebar } from "../model/settings-sidebar"
import {
  dekOf,
  settingsCategories,
  settingsCategory,
  settingsResultKey,
  type SettingsCategoryId,
  type SettingsTab,
  type SettingsTabId,
} from "../model/settings-catalogue"
import { DesktopIcon, type DesktopIconRole } from "../../ui/icons"
import { FoundSetting } from "./settings-controls"
import { SettingsMasthead } from "./settings-masthead"
import { settingsTabPages } from "./settings-tabs"
import type { SettingsSurfaceModel } from "./use-settings-surface"
import "./settings.css"
import { tooltip } from "../../ui/tooltip"

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

export function SettingsLayout(model: SettingsSurfaceModel) {
  const {
    hostKind,
    browserSurface,
    theme,
    onClose,
    rootRef,
    toggleRef,
    sidebarRef,
    scrollRef,
    searchRef,
    resultsRef,
    contentRef,
    sidebarOpen,
    revealed,
    peek,
    toggleSidebar,
    revealSidebar,
    sidebarAction,
    sidebarShortcut,
    category,
    chooseCategory,
    tab,
    tabs,
    tabsShown,
    query,
    setQuery,
    results,
    found,
    picked,
    showTab,
    go,
    moveInResults,
  } = model
  const Page = settingsTabPages[tab]
  const sidebarLabel = `${sidebarAction} (${sidebarShortcut})`
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
          {...tooltip(sidebarAction, { shortcut: sidebarShortcut })}
          aria-expanded={sidebarOpen}
          aria-controls="settings-sidebar"
          onClick={toggleSidebar}
        >
          <DesktopIcon name="sidebar" />
        </button>
        <HistoryButtons />
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

      {sidebarOpen ? null : <EdgePeekStrip peek={peek} onDragOut={revealSidebar} />}
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
                <li key={settingsResultKey(match)}>
                  <button
                    type="button"
                    className="settings-nav-item settings-result"
                    aria-current={
                      settingsResultKey(match) === picked ? "true" : undefined
                    }
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
                    onClick={() => chooseCategory(item.id)}
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
          <IdentityRow as="div" mark="none">
            <IdentityButton
              product="Agent"
              back
              label="Back to nessa Agent"
              onClick={onClose}
            />
          </IdentityRow>
        </nav>
      </div>

      <main
        ref={contentRef}
        className="settings-content"
        aria-labelledby="settings-heading"
      >
        {/* The page's drag strip; its title shows once the masthead's has scrolled away. */}
        <div className="settings-bar" data-tauri-drag-region>
          <span className="settings-bar-title" aria-hidden="true">
            {settingsCategory(category).label}
          </span>
        </div>
        <div ref={scrollRef} className="settings-scroll">
          <SettingsPage
            key={category}
            category={category}
            tab={tab}
            tabs={tabsShown ? tabs : undefined}
            onTab={showTab}
          >
            <FoundSetting.Provider value={found}>
              <Page />
            </FoundSetting.Provider>
          </SettingsPage>
        </div>
      </main>
    </div>
  )
}

const tabButtonId = (tab: SettingsTabId) => `settings-tab-${tab}`

/**
 * One category's page in the centred column: its masthead — the title, and
 * the strip of tabs under it when it has several — then the open tab's
 * settings, the strip's panel.
 */
function SettingsPage({
  category,
  tab,
  tabs,
  onTab,
  children,
}: {
  category: SettingsCategoryId
  tab: SettingsTabId
  tabs: readonly SettingsTab[] | undefined
  onTab: (tab: SettingsTab) => void
  children: ReactNode
}) {
  return (
    <div className="settings-column settings-page">
      <SettingsMasthead
        title={settingsCategory(category).label}
        dek={dekOf(category)}
        headingId="settings-heading"
      >
        {tabs ? <SettingsTabStrip tabs={tabs} selected={tab} onSelect={onTab} /> : null}
      </SettingsMasthead>
      <div
        key={tab}
        id="settings-panel"
        className="settings-panel"
        role={tabs ? "tabpanel" : undefined}
        aria-labelledby={tabs ? tabButtonId(tab) : undefined}
      >
        {children}
      </div>
    </div>
  )
}

/**
 * Places the tab pill by measurement, written straight to the strip's style
 * so moving it is not a render. Only after the first placement does the pill
 * glide, so it never slides in from nowhere.
 */
function useSettingsTabPill(
  listRef: RefObject<HTMLDivElement | null>,
  selected: SettingsTabId,
) {
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
    const frame = requestAnimationFrame(() => list.setAttribute("data-placed", ""))
    const observer = new ResizeObserver(place)
    observer.observe(list)
    return () => {
      cancelAnimationFrame(frame)
      observer.disconnect()
    }
  }, [listRef, selected])
}

/**
 * The tabs across the top of a category's page. One tab stop: the arrow keys,
 * Home, and End move between tabs and show each as it is reached.
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
  useSettingsTabPill(listRef, selected)

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
