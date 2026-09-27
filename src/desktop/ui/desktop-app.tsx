import { useEffect, useRef, useState, type PointerEvent } from "react"
import {
  ArrowLeft,
  ArrowRight,
  Home as HomeIcon,
  Maximize2,
  Minimize2,
  PanelLeft,
  PanelRight,
} from "lucide-react"
import { AppShell, AppShellBody, AppShellMain } from "@nessa-ui/react/app-shell"
import { Button } from "@nessa-ui/react/button"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from "@nessa-ui/react/sidebar"
import { SplitView, SplitViewPanel, SplitViewSeparator } from "@nessa-ui/react/split-view"
import type { HostKind } from "../../host/features"
import { useEdgePeek } from "../adapters/use-edge-peek"
import { useThemePreference } from "../adapters/theme-preference"
import type { DesktopThemeId } from "../model/theme"
import {
  LEFT_DEFAULT_WIDTH,
  LEFT_MAX_WIDTH,
  LEFT_MIN_WIDTH,
  RIGHT_DEFAULT_WIDTH,
  WORKSPACE_MIN_WIDTH,
} from "../adapters/sidebar-sizing"
import { useSidebarLayout } from "../adapters/use-sidebar-layout"
import { Home } from "./home"
import { ThemeMenu } from "./theme-menu"
import { WindowTitlebar } from "./window-titlebar"

/** Track only the local glow position; SplitView continues to own dragging. */
function positionEdgeGlow(event: PointerEvent<HTMLDivElement>) {
  const edge = event.currentTarget
  const y = event.clientY - edge.getBoundingClientRect().top
  edge.style.setProperty("--edge-glow-y", `${y}px`)
}

const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)
const shortcut = (keys: string) =>
  isMac ? keys : keys.replace("⌥", "Alt+").replace("⌘", "Ctrl+")

/** Composes the shell, sidebars, and split view without product/backend state. */
export function DesktopApp({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const {
    groupRef,
    layout,
    changeLayout,
    setOpen,
    leftOpen,
    rightOpen,
    beginResize,
    rightMinWidth,
    snapWorkspace,
    workspaceCollapsed,
  } = useSidebarLayout()
  const [theme, setTheme] = useThemePreference()
  const [rightMaximized, setRightMaximized] = useState(false)
  // Widths animate only once the first measured layout has painted; before
  // that, panels would visibly slide from equal thirds into place on load.
  const [settled, setSettled] = useState(false)
  useEffect(() => {
    if (!layout || settled) return
    const frame = requestAnimationFrame(() => setSettled(true))
    return () => cancelAnimationFrame(frame)
  }, [layout, settled])
  const rightShown = rightOpen || rightMaximized
  const leftDocked = leftOpen && !rightMaximized
  const peek = useEdgePeek(!leftOpen && !rightMaximized)

  const toggleRight = () => {
    setRightMaximized(false)
    setOpen("right", rightMaximized ? false : !rightOpen)
  }

  // ⌥⌘B mirrors the left sidebar's ⌘B for the panel on the other side, and
  // Esc leaves the maximized panel wherever focus happens to be.
  const keysRef = useRef({ toggleRight, rightMaximized })
  keysRef.current = { toggleRight, rightMaximized }
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const mod = isMac ? event.metaKey : event.ctrlKey
      if (mod && event.altKey && event.code === "KeyB") {
        event.preventDefault()
        keysRef.current.toggleRight()
      } else if (event.key === "Escape" && keysRef.current.rightMaximized) {
        event.preventDefault()
        setRightMaximized(false)
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])

  const leftLabel = `${leftDocked ? "Hide" : "Show"} Sidebar (${shortcut("⌘B")})`
  const rightLabel = `${rightShown ? "Hide" : "Show"} Panel (${shortcut("⌥⌘B")})`
  const maximizeLabel = rightMaximized ? "Restore Panel (Esc)" : "Expand Panel"

  const maximizeButton = (
    <Button
      variant="ghost"
      size="icon"
      className="desktop-titlebar-button desktop-maximize"
      aria-label={maximizeLabel}
      title={maximizeLabel}
      aria-pressed={rightMaximized}
      aria-controls="right"
      onClick={() => setRightMaximized((value) => !value)}
    >
      {rightMaximized ? <Minimize2 /> : <Maximize2 />}
    </Button>
  )

  const rightToggle = (
    <Button
      variant="ghost"
      size="icon"
      className="desktop-titlebar-button"
      aria-label={rightLabel}
      aria-expanded={rightShown}
      aria-controls="right"
      title={rightLabel}
      onClick={toggleRight}
    >
      <PanelRight />
    </Button>
  )

  return (
    <SidebarProvider
      data-host={hostKind}
      data-surface={browserSurface ? "browser" : "window"}
      data-desktop-theme={theme}
      open={leftOpen && !rightMaximized}
      onOpenChange={(open) => {
        setRightMaximized(false)
        setOpen("left", open)
      }}
      sidebarWidth="100%"
      keyboardShortcut={{ key: "b", modifier: "mod" }}
    >
      <AppShell
        className="desktop-shell relative h-svh w-full min-w-[350px]"
        data-right-maximized={rightMaximized || undefined}
        maximizeShortcut={false}
      >
        <div className="desktop-ambient" aria-hidden="true">
          <span className="desktop-grain" />
        </div>
        {/* One titlebar for every surface, outside the split view, so no
            toggle moves when a sidebar opens or closes. */}
        <WindowTitlebar
          className="desktop-titlebar absolute inset-x-0 top-0 z-20"
          style={{ background: "transparent" }}
          data-tauri-drag-region
          windowControlsInset="var(--desktop-window-controls-inset)"
          height="var(--desktop-titlebar-height)"
          leading={
            <SidebarTrigger
              className="desktop-titlebar-button"
              aria-label={leftLabel}
              title={leftLabel}
              aria-expanded={leftDocked}
              aria-controls="left"
            >
              <PanelLeft />
            </SidebarTrigger>
          }
          navigation={{
            back: { label: "Go back", icon: <ArrowLeft />, disabled: true },
            forward: { label: "Go forward", icon: <ArrowRight />, disabled: true },
          }}
          trailing={rightToggle}
        />
        {/* Dock-style reveal: resting on the left edge while the sidebar is
            collapsed slides it in over the content until the pointer leaves. */}
        {!leftOpen && !rightMaximized ? (
          <div
            className="desktop-peek-edge"
            aria-hidden="true"
            onPointerEnter={peek.enter}
            onPointerLeave={peek.leave}
          />
        ) : null}
        <div
          className="desktop-peek"
          data-shown={peek.shown || undefined}
          inert={!peek.shown}
          aria-hidden={!peek.shown}
          onPointerEnter={peek.enter}
          onPointerLeave={peek.leave}
        >
          <Sidebar
            aria-label="Main navigation"
            collapsible="none"
            className="desktop-sidebar desktop-glass"
          >
            <NavigationBody theme={theme} onThemeChange={setTheme} />
          </Sidebar>
        </div>
        <AppShellBody className="bg-transparent">
          <SplitView
            ref={groupRef}
            className="desktop-split h-full w-full"
            data-settled={settled || undefined}
            layout={layout}
            onLayoutChange={changeLayout}
          >
            <SplitViewPanel
              id="left"
              minSize={`${LEFT_MIN_WIDTH}px`}
              defaultSize={`${LEFT_DEFAULT_WIDTH}px`}
              maxSize={`${LEFT_MAX_WIDTH}px`}
              collapsible
              collapsedSize={0}
              inert={!leftOpen || rightMaximized}
              aria-hidden={!leftOpen || rightMaximized}
            >
              <Sidebar
                aria-label="Main navigation"
                collapsible="none"
                className="desktop-sidebar desktop-glass"
              >
                <NavigationBody theme={theme} onThemeChange={setTheme} />
              </Sidebar>
            </SplitViewPanel>
            <SplitViewSeparator
              className="desktop-sidebar-edge"
              onPointerEnter={positionEdgeGlow}
              onPointerMove={positionEdgeGlow}
              onFocus={(event) =>
                event.currentTarget.style.removeProperty("--edge-glow-y")
              }
              onPointerDownCapture={() => beginResize("left")}
              onKeyDownCapture={() => beginResize("left")}
              aria-label="Resize left sidebar"
            />
            <SplitViewPanel
              id="center"
              minSize={`${WORKSPACE_MIN_WIDTH}px`}
              collapsible
              collapsedSize={0}
              inert={rightMaximized || workspaceCollapsed}
              aria-hidden={rightMaximized || workspaceCollapsed}
            >
              <AppShellMain id="home" aria-label="Home" className="desktop-main">
                <Home />
              </AppShellMain>
            </SplitViewPanel>
            <SplitViewSeparator
              className="desktop-sidebar-edge"
              onPointerEnter={positionEdgeGlow}
              onPointerMove={(event) => {
                positionEdgeGlow(event)
                snapWorkspace(event)
              }}
              onFocus={(event) =>
                event.currentTarget.style.removeProperty("--edge-glow-y")
              }
              onPointerDownCapture={() => beginResize("right")}
              onKeyDownCapture={() => beginResize("right")}
              aria-label="Resize right sidebar"
            />
            <SplitViewPanel
              id="right"
              defaultSize={`${RIGHT_DEFAULT_WIDTH}px`}
              minSize={`${rightMinWidth}px`}
              collapsible
              collapsedSize={0}
              inert={!rightShown}
              aria-hidden={!rightShown}
            >
              <SidebarProvider
                open={rightShown}
                onOpenChange={(open) => setOpen("right", open)}
                sidebarWidth="100%"
                className="desktop-right-content h-full min-h-0"
              >
                <Sidebar
                  side="right"
                  aria-label="Right sidebar"
                  collapsible="none"
                  className="desktop-sidebar desktop-glass"
                >
                  {maximizeButton}
                  <SidebarContent className="items-center justify-center">
                    <p className="desktop-empty-note text-center">Nothing open</p>
                  </SidebarContent>
                </Sidebar>
              </SidebarProvider>
            </SplitViewPanel>
          </SplitView>
        </AppShellBody>
      </AppShell>
    </SidebarProvider>
  )
}

/** The left sidebar's contents, shared by the docked sidebar and its edge reveal. */
function NavigationBody({
  theme,
  onThemeChange,
}: {
  theme: DesktopThemeId
  onThemeChange: (theme: DesktopThemeId) => void
}) {
  return (
    <>
      <SidebarContent>
        <SidebarMenu>
          <SidebarMenuItem asChild icon={<HomeIcon />} isActive>
            <a href="#home" aria-current="page">
              Home
            </a>
          </SidebarMenuItem>
        </SidebarMenu>
        <SidebarGroup className="mt-4">
          <SidebarGroupLabel>Recents</SidebarGroupLabel>
          <SidebarGroupContent>
            <p className="desktop-empty-note">Your conversations will appear here.</p>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter className="desktop-identity">
        <span aria-hidden="true" className="desktop-mark" />
        <span className="min-w-0 flex-1 truncate">
          <span className="font-semibold">nessa</span>
          <span className="font-normal text-muted-foreground">Studio</span>
        </span>
        <ThemeMenu theme={theme} onThemeChange={onThemeChange} />
      </SidebarFooter>
    </>
  )
}
