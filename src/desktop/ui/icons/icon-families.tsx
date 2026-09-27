import type { ReactNode } from "react"
import { useIconFamilyPreference } from "../../adapters/icon-family-preference"
import type { IconFamilyId } from "../../model/icon-family"
import type { DesktopIconFamily } from "./icon-contract"
import { DesktopIconProvider } from "./icon-provider"
import { lucideIcons } from "./lucide-icons"
import { nessaIcons } from "./nessa-icons"

/** Each family's drawings, for every family `model/icon-family.ts` names. */
export const iconFamilyDrawings: Record<IconFamilyId, DesktopIconFamily> = {
  nessa: nessaIcons,
  lucide: lucideIcons,
}

/**
 * The window's root icon provider: everything beneath it draws in the family
 * chosen in Settings › Appearance, and follows a change at once.
 */
export function DesktopIconFamilyProvider({ children }: { children: ReactNode }) {
  const [family] = useIconFamilyPreference()
  return (
    <DesktopIconProvider icons={iconFamilyDrawings[family]}>
      {children}
    </DesktopIconProvider>
  )
}
