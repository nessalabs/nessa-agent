/**
 * Settings' orchestration: the sidebar's fold, its chords, which category
 * and tab are showing, search, and where focus and the scroll go.
 * `SettingsLayout` renders the result. See
 * `docs/design/workspace-shell-split.md`.
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react"
import type { HostKind } from "../../../host/features"
import { holdStill, slideFrom } from "../../adapters/hold-still"
import { isMac } from "../../adapters/platform"
import { chordLabel, commandKey, matchesChord, type Chord } from "../../model/keyboard"
import { reducedMotion } from "../../adapters/motion-preference"
import { useThemePreference } from "../../adapters/theme-preference"
import { useEdgePeek, type EdgePeekControls } from "../../adapters/use-edge-peek"
import { useWindowWidth } from "../../adapters/window-width"
import { chosen, drawn, fitted, type SideColumn } from "../../model/side-column"
import type { DesktopThemeId } from "../../model/theme"
import { focusInFront } from "../../workspace"
import { settingsSidebar, settingsSidebarFits } from "../model/settings-sidebar"
import {
  firstTabOf,
  searchSettings,
  settingsTab,
  showsTabs,
  tabsOf,
  type SettingId,
  type SettingsCategoryId,
  settingsResultKey,
  type SettingsMatch,
  type SettingsTab,
  type SettingsTabId,
} from "../model/settings-catalogue"

/** ⌘B shows and hides Settings' sidebar, as it does the window's. */
const sidebarChord: Chord = { code: "KeyB", command: true }

/** ⌘F goes to Settings' search, as it does in the system's own settings. */
const searchChord: Chord = { code: "KeyF", command: true }

export interface SettingsSurfaceModel {
  hostKind: HostKind
  browserSurface: boolean
  theme: DesktopThemeId
  onClose: () => void
  rootRef: RefObject<HTMLDivElement | null>
  toggleRef: RefObject<HTMLButtonElement | null>
  sidebarRef: RefObject<HTMLDivElement | null>
  scrollRef: RefObject<HTMLDivElement | null>
  searchRef: RefObject<HTMLInputElement | null>
  resultsRef: RefObject<HTMLUListElement | null>
  contentRef: RefObject<HTMLElement | null>
  sidebarOpen: boolean
  revealed: boolean
  peek: EdgePeekControls
  toggleSidebar: () => void
  revealSidebar: () => void
  sidebarAction: string
  sidebarShortcut: string
  category: SettingsCategoryId
  chooseCategory: (id: SettingsCategoryId) => void
  tab: SettingsTabId
  tabs: readonly SettingsTab[]
  tabsShown: boolean
  query: string
  setQuery: (query: string) => void
  results: readonly SettingsMatch[]
  found: SettingId | null
  picked: string | null
  showTab: (tab: SettingsTab) => void
  go: (match: SettingsMatch) => void
  moveInResults: (event: ReactKeyboardEvent, index: number) => void
}

export function useSettingsSurface({
  hostKind,
  browserSurface,
  tab: openedOn,
  onClose,
}: {
  hostKind: HostKind
  browserSurface: boolean
  tab?: SettingsTabId
  onClose: () => void
}): SettingsSurfaceModel {
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
  const revealSidebar = useCallback(
    () => setSidebarColumn((column) => chosen(column, true)),
    [setSidebarColumn],
  )
  // Folded, the sidebar can be revealed from the window's edge, as the app's can.
  const rootRef = useRef<HTMLDivElement>(null)
  // Settings is a dialog itself: its own reveal is not under one.
  const peek = useEdgePeek(!sidebarOpen, sidebarOpen, rootRef)
  const revealed = peek.shown && !peek.handingOff
  const [category, setCategory] = useState<SettingsCategoryId>(() =>
    openedOn ? settingsTab(openedOn).category : "appearance",
  )
  // The tab last shown in each category, kept while Settings is open.
  const [chosenTabs, setChosenTabs] = useState<
    Partial<Record<SettingsCategoryId, SettingsTabId>>
  >(() => (openedOn ? { [settingsTab(openedOn).category]: openedOn } : {}))
  const [query, setQuery] = useState("")
  // The setting a search just jumped to, shown briefly on its row.
  const [found, setFound] = useState<SettingId | null>(null)
  // The search result last taken, marked in the results while they stay up.
  const [picked, setPicked] = useState<string | null>(null)
  const toggleRef = useRef<HTMLButtonElement>(null)
  const sidebarRef = useRef<HTMLDivElement>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const resultsRef = useRef<HTMLUListElement>(null)
  const contentRef = useRef<HTMLElement>(null)
  // ⌘F with the sidebar away shows it, then searches once it is there.
  const [searchAsked, setSearchAsked] = useState(false)

  const tabs = tabsOf(category)
  const tabsShown = showsTabs(category)
  const tab = chosenTabs[category] ?? firstTabOf(category)
  const results = useMemo(() => searchSettings(query), [query])

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
      if (matchesChord(event, searchChord, isMac)) {
        event.preventDefault()
        if (sidebarOpen) {
          searchRef.current?.focus()
          searchRef.current?.select()
        } else {
          setSidebarColumn((column) => chosen(column))
          setSearchAsked(true)
        }
        return
      }
      if (!matchesChord(event, sidebarChord, isMac)) return
      event.preventDefault()
      toggleSidebar()
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [toggleSidebar, sidebarOpen, setSidebarColumn])
  useEffect(() => {
    if (!searchAsked || !sidebarOpen) return
    setSearchAsked(false)
    searchRef.current?.focus()
  }, [searchAsked, sidebarOpen])

  // The sidebar's room changes at once, and the page slides into its new
  // place by transform, as the workspace's columns do; the bar's title holds
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
    const title = content.querySelector<HTMLElement>(":scope > .settings-bar > *")
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

  // A new page starts at its top; a jump to a setting brings that setting
  // to the middle of the view, where the eye is.
  useLayoutEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 })
  }, [category, tab])
  useEffect(() => {
    if (!found) return
    const row = scrollRef.current?.querySelector(`[data-setting="${found}"]`)
    row?.scrollIntoView({
      block: "center",
      behavior: reducedMotion() ? "auto" : "smooth",
    })
    const timer = window.setTimeout(() => setFound(null), 1600)
    return () => window.clearTimeout(timer)
  }, [found])

  // Once the masthead's title has scrolled under the bar, the bar shows the
  // page's name small. Read on scroll, at most once a frame, and written
  // straight to the page's attribute, so a scroll is not a render.
  useEffect(() => {
    const scroller = scrollRef.current
    const content = contentRef.current
    const title = scroller?.querySelector(".settings-title")
    if (!scroller || !content || !title) return
    let frame = 0
    const update = () => {
      frame = 0
      const under =
        title.getBoundingClientRect().bottom <= scroller.getBoundingClientRect().top
      content.toggleAttribute("data-condensed", under)
    }
    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(update)
    }
    update()
    scroller.addEventListener("scroll", onScroll, { passive: true })
    return () => {
      scroller.removeEventListener("scroll", onScroll)
      cancelAnimationFrame(frame)
      content.removeAttribute("data-condensed")
    }
  }, [category])

  const showTab = (next: SettingsTab) => {
    setChosenTabs((all) => ({ ...all, [next.category]: next.id }))
  }

  const go = (match: SettingsMatch) => {
    setCategory(match.category)
    setChosenTabs((all) => ({ ...all, [match.category]: match.tab }))
    setFound(match.setting ?? null)
    setPicked(settingsResultKey(match))
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
  return {
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
    sidebarShortcut: chordLabel(sidebarChord, isMac),
    category,
    chooseCategory: setCategory,
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
  }
}
