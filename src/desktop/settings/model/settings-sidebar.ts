/**
 * Settings' sidebar and the window's width: its width, and the least room
 * a page of settings needs beside it. A window narrower than both folds the
 * sidebar for room (`model/side-column.ts`), and it comes back when the
 * window widens again, unless the person hid it. The stylesheet draws the
 * width this module says.
 */
export const settingsSidebar = {
  /** The sidebar's width, its gutter included. */
  width: 248,
  /** The narrowest a page of settings reads well: its rows keep their controls beside them. */
  minContent: 420,
} as const

/** Whether a window `windowWidth` wide has room for the sidebar beside a page. */
export function settingsSidebarFits(windowWidth: number): boolean {
  return windowWidth - settingsSidebar.width >= settingsSidebar.minContent
}
