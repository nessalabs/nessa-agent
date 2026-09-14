import type { PointerEvent } from "react"
import { ArrowLeft, ArrowRight, Home, PanelLeft, PanelRight } from "lucide-react"
import { AppShell, AppShellBody, AppShellMain } from "@nessa-ui/react/app-shell"
import { Button } from "@nessa-ui/react/button"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from "@nessa-ui/react/sidebar"
import { SplitView, SplitViewPanel, SplitViewSeparator } from "@nessa-ui/react/split-view"
import type { HostKind } from "../../host/features"
import { useSidebarLayout } from "../adapters/use-sidebar-layout"
import { BrowserTitlebar } from "./browser-titlebar"
import { WindowTitlebar } from "./window-titlebar"

/** Track only the local glow position; SplitView continues to own dragging. */
function positionEdgeGlow(event: PointerEvent<HTMLDivElement>) {
  const edge = event.currentTarget
  const y = event.clientY - edge.getBoundingClientRect().top
  edge.style.setProperty("--edge-glow-y", `${y}px`)
}

/** Composes the existing shell, sidebars, and split view without product/backend state. */
export function DesktopApp({
  hostKind,
  browserSurface,
}: {
  hostKind: HostKind
  browserSurface: boolean
}) {
  const { groupRef, layout, changeLayout, setOpen, leftOpen, rightOpen } =
    useSidebarLayout()
  const rightToggle = (
    <Button
      variant="ghost"
      size="icon"
      className="size-8 text-muted-foreground"
      aria-label="Toggle right sidebar"
      aria-expanded={rightOpen}
      aria-controls="right"
      title="Toggle right sidebar"
      onClick={() => setOpen("right", !rightOpen)}
    >
      <PanelRight />
    </Button>
  )

  return (
    <SidebarProvider
      data-host={hostKind}
      open={leftOpen}
      onOpenChange={(open) => setOpen("left", open)}
      sidebarWidth="100%"
      keyboardShortcut={{ key: "b", modifier: "mod" }}
    >
      <AppShell className="relative h-svh w-full min-w-[350px]" maximizeShortcut={false}>
        {browserSurface ? (
          <BrowserTitlebar trailing={rightToggle} />
        ) : (
          <WindowTitlebar
            className="absolute inset-x-0 top-0 z-20"
            style={{ background: "transparent" }}
            data-tauri-drag-region
            windowControlsInset="var(--desktop-window-controls-inset, 8px)"
            height={42}
            leading={
              <SidebarTrigger aria-expanded={leftOpen} aria-controls="left">
                <PanelLeft />
              </SidebarTrigger>
            }
            navigation={{
              back: { label: "Go back", icon: <ArrowLeft />, disabled: true },
              forward: { label: "Go forward", icon: <ArrowRight />, disabled: true },
            }}
            trailing={rightToggle}
          />
        )}
        <AppShellBody>
          <SplitView
            ref={groupRef}
            className="desktop-split h-full w-full"
            layout={layout}
            onLayoutChange={changeLayout}
          >
            <SplitViewPanel
              id="left"
              minSize="200px"
              defaultSize="200px"
              maxSize="450px"
              collapsible
              collapsedSize={0}
              inert={!leftOpen}
              aria-hidden={!leftOpen}
            >
              <Sidebar
                aria-label="Main navigation"
                collapsible="none"
                className={`desktop-sidebar ${browserSurface ? "" : "pt-[42px]"}`}
              >
                <SidebarHeader className="flex h-14 shrink-0 flex-row items-center gap-2 px-5 py-0">
                  <span
                    aria-hidden="true"
                    className="size-1.5 shrink-0 rounded-[1px] bg-foreground"
                  />
                  <span className="text-base font-semibold tracking-tight">nessa</span>
                </SidebarHeader>
                <SidebarContent>
                  <SidebarMenu>
                    <SidebarMenuItem asChild icon={<Home />} isActive>
                      <a href="#home" aria-current="page">
                        Home
                      </a>
                    </SidebarMenuItem>
                  </SidebarMenu>
                </SidebarContent>
                <SidebarFooter className="flex-row items-center justify-between border-t border-border px-3 py-2">
                  <span className="flex min-w-0 items-center gap-2 text-sm tracking-tight">
                    <span
                      aria-hidden="true"
                      className="size-1.5 shrink-0 rounded-[1px] bg-foreground"
                    />
                    <span className="truncate">
                      <span className="font-semibold">nessa</span>
                      <span className="font-normal text-muted-foreground">Studio</span>
                    </span>
                  </span>
                  {browserSurface && (
                    <SidebarTrigger
                      aria-label="Close sidebar"
                      title="Close sidebar"
                      aria-expanded={true}
                      aria-controls="left"
                    >
                      <PanelLeft />
                    </SidebarTrigger>
                  )}
                </SidebarFooter>
              </Sidebar>
            </SplitViewPanel>
            <SplitViewSeparator
              className="desktop-sidebar-edge"
              onPointerEnter={positionEdgeGlow}
              onPointerMove={positionEdgeGlow}
              onFocus={(event) =>
                event.currentTarget.style.removeProperty("--edge-glow-y")
              }
              aria-label="Resize left sidebar"
            />
            <SplitViewPanel id="center" minSize="350px">
              <AppShellMain
                id="home"
                aria-label="Home"
                className={browserSurface ? "pt-14" : "pt-[42px]"}
              />
            </SplitViewPanel>
            <SplitViewSeparator
              className="desktop-sidebar-edge"
              onPointerEnter={positionEdgeGlow}
              onPointerMove={positionEdgeGlow}
              onFocus={(event) =>
                event.currentTarget.style.removeProperty("--edge-glow-y")
              }
              aria-label="Resize right sidebar"
            />
            <SplitViewPanel
              id="right"
              defaultSize="400px"
              minSize="160px"
              collapsible
              collapsedSize={0}
              inert={!rightOpen}
              aria-hidden={!rightOpen}
            >
              <SidebarProvider
                open={rightOpen}
                onOpenChange={(open) => setOpen("right", open)}
                sidebarWidth="100%"
                className="h-full min-h-0"
              >
                <Sidebar
                  side="right"
                  aria-label="Right sidebar"
                  collapsible="none"
                  className={`desktop-sidebar ${browserSurface ? "pt-14" : "pt-[42px]"}`}
                >
                  <SidebarContent />
                </Sidebar>
              </SidebarProvider>
            </SplitViewPanel>
          </SplitView>
        </AppShellBody>
      </AppShell>
    </SidebarProvider>
  )
}
