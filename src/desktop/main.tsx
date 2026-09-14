import * as React from "react"
import { createRoot } from "react-dom/client"
import { ArrowLeft, ArrowRight, Home, PanelLeft } from "lucide-react"
import { AppShell, AppShellBody, AppShellMain } from "@nessa-ui/react/app-shell"
import {
  Sidebar,
  SidebarContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from "@nessa-ui/react/sidebar"

import { host } from "../host"
import { BrowserSidebarToggle, BrowserTitlebar } from "./ui/browser-titlebar"
import { WindowTitlebar } from "./ui/window-titlebar"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

const browserSurface = host.kind === "browser"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <SidebarProvider
      data-host={host.kind}
      keyboardShortcut={{ key: "b", modifier: "mod" }}
    >
      <AppShell className="h-svh w-full" maximizeShortcut={false}>
        {browserSurface ? (
          <BrowserTitlebar />
        ) : (
          <WindowTitlebar
            data-tauri-drag-region
            windowControlsInset="var(--desktop-window-controls-inset, 8px)"
            height={42}
            leading={
              <SidebarTrigger>
                <PanelLeft />
              </SidebarTrigger>
            }
            navigation={{
              back: { label: "Go back", icon: <ArrowLeft />, disabled: true },
              forward: { label: "Go forward", icon: <ArrowRight />, disabled: true },
            }}
          />
        )}
        <AppShellBody>
          <Sidebar aria-label="Main navigation" className="h-full">
            {browserSurface && (
              <SidebarHeader className="px-3 py-2">
                <BrowserSidebarToggle />
              </SidebarHeader>
            )}
            <SidebarContent>
              <SidebarMenu>
                <SidebarMenuItem asChild icon={<Home />} isActive>
                  <a href="#home" aria-current="page">
                    Home
                  </a>
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarContent>
          </Sidebar>
          <AppShellMain id="home" aria-label="Home" />
        </AppShellBody>
      </AppShell>
    </SidebarProvider>
  </React.StrictMode>,
)
