import type { ReactNode } from "react"
import { PanelLeft } from "lucide-react"
import { SidebarTrigger, useSidebar } from "@nessa-ui/react/sidebar"

/** Browser overlay keeps reopening and right-panel controls anchored to the window. */
export function BrowserTitlebar({ trailing }: { trailing?: ReactNode }) {
  const { open } = useSidebar()
  return (
    <header className="pointer-events-none absolute inset-x-0 top-0 z-20 flex h-14 items-center px-3 [&_button]:pointer-events-auto">
      {!open && (
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
