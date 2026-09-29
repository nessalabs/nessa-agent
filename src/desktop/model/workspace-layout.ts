/**
 * How the window lays out its workspace, chosen in Settings › Workspace ›
 * Layout: three columns (sidebar, a channel's session list, chat), sessions
 * listed under their channels in the sidebar, or the classic shell with a
 * home and a right panel.
 */
export const workspaceLayouts = [
  { id: "columns", label: "Three columns" },
  { id: "sidebar", label: "Sessions in sidebar" },
  { id: "classic", label: "Classic" },
] as const

export type WorkspaceLayoutId = (typeof workspaceLayouts)[number]["id"]

export const defaultWorkspaceLayout: WorkspaceLayoutId = "columns"

/** Reads a stored or requested layout, falling back to the default for anything unknown. */
export function parseWorkspaceLayout(value: unknown): WorkspaceLayoutId {
  return (
    workspaceLayouts.find((layout) => layout.id === value)?.id ?? defaultWorkspaceLayout
  )
}
