import type { ReactNode } from "react"
import { PanelLeft } from "lucide-react"
import { SidebarTrigger, useSidebar } from "@nessa-ui/react/sidebar"
import nessaLogo from "../../../src-tauri/icons/nessa-icon.svg"

/** Browser chrome shows identity while open and a plain disclosure while closed. */
export function BrowserTitlebar({ trailing }: { trailing?: ReactNode }) {
  const { open } = useSidebar()
  return (
    <header className="flex h-14 shrink-0 items-center px-3">
      {open ? (
        <div className="flex items-center gap-2 px-2 text-lg font-semibold">
          <img src={nessaLogo} alt="" className="size-6 rounded-md" />
          <span>Nessa</span>
        </div>
      ) : (
        <SidebarTrigger
          aria-label="Open sidebar"
          title="Open sidebar"
          aria-expanded={false}
          aria-controls="left"
        >
          <PanelLeft />
        </SidebarTrigger>
      )}
      <div className="ml-auto">{trailing}</div>
    </header>
  )
}
