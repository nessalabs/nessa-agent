import * as React from "react"
import { createRoot } from "react-dom/client"
import { ArrowLeft, ArrowRight, Home, PanelLeft } from "lucide-react"
import {
  AppShell,
  AppShellBody,
  AppShellHeader,
  AppShellMain,
} from "@nessa-ui/react/app-shell"
import {
  Sidebar,
  SidebarContent,
  SidebarMenu,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
} from "@nessa-ui/react/sidebar"

import { host } from "../host"

import "@fontsource-variable/geist"
import "@fontsource-variable/geist-mono"
import "./styles.css"

const container = document.getElementById("root")
if (!container) throw new Error("missing #root")

createRoot(container).render(
  <React.StrictMode>
    <SidebarProvider
      className="dark"
      data-host={host.kind}
      keyboardShortcut={{ key: "b", modifier: "mod" }}
    >
      <AppShell className="h-svh w-full" maximizeShortcut={false}>
        <AppShellHeader className="desktop-titlebar" data-tauri-drag-region>
          <SidebarTrigger>
            <PanelLeft />
          </SidebarTrigger>
          <button className="desktop-navigation" aria-label="Go back" disabled>
            <ArrowLeft aria-hidden="true" />
          </button>
          <button className="desktop-navigation" aria-label="Go forward" disabled>
            <ArrowRight aria-hidden="true" />
          </button>
        </AppShellHeader>
        <AppShellBody>
          <Sidebar aria-label="Main navigation" className="h-full">
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
