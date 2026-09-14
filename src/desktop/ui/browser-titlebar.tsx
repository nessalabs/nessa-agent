import { PanelLeft } from "lucide-react"
import { SidebarTrigger, useSidebar } from "@nessa-ui/react/sidebar"

import nessaLogo from "../../../src-tauri/icons/nessa-icon.svg"

/** Browser identity doubles as navigation disclosure, including inside the mobile drawer. */
export function BrowserSidebarToggle() {
  const { open } = useSidebar()

  return (
    <SidebarTrigger
      aria-label={open ? "Close sidebar" : "Open sidebar"}
      aria-expanded={open}
      title={open ? "Close sidebar" : "Open sidebar"}
      className="group h-9 w-auto self-start justify-start gap-2 px-2 text-foreground"
    >
      <span className="relative inline-flex size-6 items-center justify-center">
        <img
          src={nessaLogo}
          alt=""
          className="size-6 rounded-md group-hover:opacity-0 group-focus-visible:opacity-0"
        />
        <PanelLeft
          aria-hidden="true"
          className="absolute size-4 opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100"
        />
      </span>
      {open && <span className="text-lg font-semibold">Nessa</span>}
    </SidebarTrigger>
  )
}

/** Keeps the logo reachable when navigation is closed; the open sidebar owns its header. */
export function BrowserTitlebar() {
  const { open } = useSidebar()
  if (open) return null

  return (
    <header className="flex h-14 shrink-0 items-center px-3">
      <BrowserSidebarToggle />
    </header>
  )
}
