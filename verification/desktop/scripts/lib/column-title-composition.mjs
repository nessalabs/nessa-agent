/** Current sidebar producer facts, observed without consulting the requested layout. */
export function sidebarTitleComposition(selectors) {
  const workspaces = [...document.querySelectorAll(selectors.workspace)]
  const workspace = workspaces.length === 1 ? workspaces[0] : null
  const sidebars = [...(workspace?.querySelectorAll(selectors.sidebar) ?? [])]
  const sidebar = sidebars.length === 1 ? sidebars[0] : null
  const navigations = [...(sidebar?.querySelectorAll('nav[aria-label="Channels"]') ?? [])]
  const navigation = navigations.length === 1 ? navigations[0] : null
  const rect = sidebar?.getBoundingClientRect()
  const clip = workspace?.getBoundingClientRect()
  const visible = Boolean(
    sidebar?.checkVisibility({ opacityProperty: true, visibilityProperty: true }) &&
    Math.min(rect.right, clip.right, window.innerWidth) >
      Math.max(rect.left, clip.left, 0) + 1 &&
    Math.min(rect.bottom, clip.bottom, window.innerHeight) >
      Math.max(rect.top, clip.top, 0) + 1,
  )
  return {
    workspaces: workspaces.length,
    sidebars: sidebars.length,
    sidebarId: sidebar?.id ?? null,
    variant: sidebar?.dataset.variant ?? null,
    listState: workspace?.getAttribute("data-list") ?? null,
    state: workspace?.getAttribute("data-sidebar") ?? null,
    peek: workspace?.hasAttribute("data-peek") ?? false,
    navigations: navigations.length,
    channels:
      navigation?.querySelectorAll('[data-row="channel"][data-channel][aria-expanded]')
        .length ?? 0,
    branches: navigation?.querySelectorAll(".workspace-branch").length ?? 0,
    sessions: navigation?.querySelectorAll("[data-session-row]").length ?? 0,
    sessionLists: workspace?.querySelectorAll(selectors.sessionList).length ?? 0,
    // Count hidden real headers too; only ColumnHeader's measuring sizer is excluded.
    titles:
      workspace?.querySelectorAll(".desktop-column-title:not(.desktop-column-sizer)")
        .length ?? 0,
    inert: sidebar?.inert ?? false,
    ariaHidden: sidebar?.getAttribute("aria-hidden") ?? null,
    visible,
  }
}

/** CHECKLIST › Column titles: positive tree composition, not an empty title query. */
export function sidebarTitleCompositionFailures(observed) {
  const counts = [
    "workspaces",
    "sidebars",
    "navigations",
    "channels",
    "branches",
    "sessions",
    "sessionLists",
    "titles",
  ]
  const identities = ["sidebarId", "variant", "listState", "state", "ariaHidden"]
  const booleans = ["peek", "inert", "visible"]
  if (
    observed === null ||
    typeof observed !== "object" ||
    Array.isArray(observed) ||
    [...counts, ...identities, ...booleans].some(
      (key) => !Object.hasOwn(observed, key),
    ) ||
    counts.some((key) => !Number.isInteger(observed[key])) ||
    booleans.some((key) => typeof observed[key] !== "boolean")
  )
    return ["sidebar composition observations are incomplete or invalid"]
  const failures = []
  if (
    observed.workspaces !== 1 ||
    observed.sidebars !== 1 ||
    observed.sidebarId !== "workspace-sidebar" ||
    observed.variant !== "tree"
  )
    failures.push("sidebar composition lacks its unique workspace and owned tree")
  if (
    observed.navigations !== 1 ||
    observed.channels < 1 ||
    observed.branches < 1 ||
    observed.sessions < 1
  )
    failures.push(
      "sidebar composition lacks the sample's Channels navigation and real channel/session tree",
    )
  if (observed.listState !== null || observed.sessionLists !== 0)
    failures.push("sidebar composition contains a separate session-list column")
  if (observed.titles !== 0)
    failures.push("sidebar composition contains a real column title")
  if (
    !["open", "closed"].includes(observed.state) ||
    ![null, "true", "false"].includes(observed.ariaHidden)
  )
    failures.push("sidebar composition has an unknown fold or accessibility state")
  const hidden = observed.ariaHidden === "true"
  if (observed.inert && !hidden)
    failures.push("sidebar composition's inert tree is exposed to accessibility")
  if (observed.state === "open" && (!observed.visible || observed.inert || hidden))
    failures.push("sidebar composition's open tree is hidden or inert")
  if (observed.state === "closed" && !observed.peek && (!hidden || observed.visible))
    failures.push(
      "sidebar composition's unrevealed folded tree is visible or exposed to accessibility",
    )
  // SourceList guards an away tree before parking makes it inert (#1294).
  // Its aria-hidden hand-off can therefore be non-inert; only an exposed
  // reveal must be visible. SourceList's parked tests own input prevention.
  if (observed.state === "closed" && observed.peek && !hidden && !observed.visible)
    failures.push("sidebar composition's exposed reveal is hidden")
  return failures
}
