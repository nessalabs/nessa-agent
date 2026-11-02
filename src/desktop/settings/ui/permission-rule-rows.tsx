import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { PermissionRule } from "@nessa/client"
import type { PermissionRulesGateway } from "../adapters/permission-rules-gateway"
import { SearchField } from "@nessa-ui/react/search-field"
import { setting } from "../model/settings-catalogue"
import { Group, ItemRow } from "./settings-controls"

/**
 * Settings › Privacy & Permissions › Access: the owner's remembered rules,
 * each one tool in one project, and a way to revoke one. With no gateway the
 * row rests. Another client can revoke a rule, so the list is read again
 * when the window regains focus or becomes visible.
 */
function revokeFailureDetail(retained: boolean | undefined): string {
  if (retained === true) return "It is still remembered."
  if (retained === false) return "It is no longer listed."
  return "The list could not be read again."
}

const PermissionRulesContext = createContext<PermissionRulesGateway | undefined>(
  undefined,
)

export function PermissionRulesProvider({
  gateway,
  children,
}: {
  gateway: PermissionRulesGateway | undefined
  children: ReactNode
}) {
  return (
    <PermissionRulesContext.Provider value={gateway}>
      {children}
    </PermissionRulesContext.Provider>
  )
}

type Phase =
  | { readonly kind: "absent" }
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly rules: readonly PermissionRule[]
      readonly revoking?: string
      readonly failed?: boolean
      readonly retained?: boolean
    }

type Request = { readonly kind: "read" } | { readonly kind: "revoke" }

export function PermissionRuleRows() {
  const gateway = useContext(PermissionRulesContext)
  const [phase, setPhase] = useState<Phase>({ kind: "absent" })
  const latest = useRef<Request>({ kind: "read" })
  useEffect(() => {
    if (!gateway) return
    let current = true
    const read = () => {
      // A revoke retains ownership through its own fresh read. Background
      // refreshes defer to that read rather than replacing its pending state.
      if (latest.current.kind === "revoke") return
      const request: Request = { kind: "read" }
      latest.current = request
      void gateway.read().then((outcome) => {
        if (!current || request !== latest.current) return
        setPhase(
          outcome.ok ? { kind: "ready", rules: outcome.rules } : { kind: "unread" },
        )
      })
    }
    setPhase({ kind: "reading" })
    read()
    const onFocus = () => read()
    const onVisible = () => {
      if (document.visibilityState === "visible") read()
    }
    window.addEventListener("focus", onFocus)
    document.addEventListener("visibilitychange", onVisible)
    return () => {
      current = false
      latest.current = { kind: "read" }
      window.removeEventListener("focus", onFocus)
      document.removeEventListener("visibilitychange", onVisible)
    }
  }, [gateway])
  const revoke = (ruleId: string) => {
    if (!gateway || phase.kind !== "ready" || latest.current.kind === "revoke") return
    const request: Request = { kind: "revoke" }
    latest.current = request
    setPhase({ kind: "ready", rules: phase.rules, revoking: ruleId })
    void gateway.revoke(ruleId, crypto.randomUUID()).then(async (outcome) => {
      if (request !== latest.current) return
      const again = await gateway.read()
      if (request !== latest.current) return
      latest.current = { kind: "read" }
      if (!outcome.ok) {
        const retained = again.ok
          ? again.rules.some((rule) => rule.id === ruleId)
          : undefined
        setPhase({
          kind: "ready",
          rules: again.ok ? again.rules : phase.rules,
          failed: true,
          retained,
        })
        return
      }
      if (again.ok) setPhase({ kind: "ready", rules: again.rules })
      else
        setPhase({
          kind: "ready",
          rules: phase.rules.filter((rule) => rule.id !== ruleId || !outcome.applied),
        })
    })
  }
  const entry = setting("remembered-rules")
  const resting =
    phase.kind === "absent"
      ? "Not available in this window."
      : phase.kind === "reading"
        ? "Reading remembered rules."
        : phase.kind === "unread"
          ? "Could not read remembered rules."
          : undefined
  return (
    <Group title={entry.label} note={entry.detail}>
      {resting !== undefined ? (
        <ItemRow
          setting="remembered-rules"
          label={entry.label}
          detail={resting}
          unavailable
        />
      ) : null}
      {phase.kind === "ready" && phase.failed ? (
        <ItemRow
          label="Could not revoke that rule."
          detail={revokeFailureDetail(phase.retained)}
        />
      ) : null}
      {phase.kind === "ready" && phase.rules.length === 0 ? (
        <ItemRow
          setting="remembered-rules"
          label={entry.label}
          detail="Rules you save from a review appear here."
        />
      ) : null}
      {phase.kind === "ready" && phase.rules.length > 0 ? (
        <RuleList
          rules={phase.rules}
          revoking={phase.revoking !== undefined}
          onRevoke={revoke}
        />
      ) : null}
    </Group>
  )
}

/** Past this many rules the list can be searched. */
const SEARCHABLE_AFTER = 6

/** The rules of one project, as the list folds them. */
type Project = { readonly workspace: string; readonly rules: readonly PermissionRule[] }

/** The last part of a folder's path: what a person calls the project. */
function projectName(workspace: string): string {
  const parts = workspace.split(/[\\/]/).filter((part) => part !== "")
  return parts.at(-1) ?? workspace
}

function ruleLabel(rule: PermissionRule): string {
  return `${rule.effect === "allow" ? "Always allow" : "Always deny"} ${rule.tool}`
}

/** Whether `rule` is one `query` asks for: its tool, agent, effect or project. */
function matches(rule: PermissionRule, query: string): boolean {
  const asked = query.trim().toLowerCase()
  if (asked === "") return true
  return [ruleLabel(rule), rule.source, rule.workspace].some((text) =>
    text.toLowerCase().includes(asked),
  )
}

/** The rules grouped by project, projects by name, rules by tool. */
function projectsOf(rules: readonly PermissionRule[]): Project[] {
  const byWorkspace = new Map<string, PermissionRule[]>()
  for (const rule of rules) {
    const listed = byWorkspace.get(rule.workspace)
    if (listed) listed.push(rule)
    else byWorkspace.set(rule.workspace, [rule])
  }
  return [...byWorkspace]
    .map(([workspace, listed]) => ({
      workspace,
      rules: [...listed].sort((a, b) => a.tool.localeCompare(b.tool)),
    }))
    .sort(
      (a, b) =>
        projectName(a.workspace).localeCompare(projectName(b.workspace)) ||
        a.workspace.localeCompare(b.workspace),
    )
}

/**
 * The remembered rules, one folded row per project that says how many it
 * holds. With one project there is nothing to fold, and past a handful of
 * rules a search narrows every project at once, opening the ones it finds.
 */
function RuleList({
  rules,
  revoking,
  onRevoke,
}: {
  rules: readonly PermissionRule[]
  revoking: boolean
  onRevoke: (ruleId: string) => void
}) {
  const [query, setQuery] = useState("")
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set())
  const searchable = rules.length > SEARCHABLE_AFTER
  const searching = searchable && query.trim() !== ""
  const projects = useMemo(
    () => projectsOf(searching ? rules.filter((rule) => matches(rule, query)) : rules),
    [rules, query, searching],
  )
  const folds = projectsOf(rules).length > 1
  const toggle = (workspace: string) =>
    setOpen((current) => {
      const next = new Set(current)
      if (next.has(workspace)) next.delete(workspace)
      else next.add(workspace)
      return next
    })
  return (
    <>
      {searchable ? (
        <ItemRow
          label="Search rules"
          className="settings-rule-search"
          control={
            <SearchField
              size="sm"
              value={query}
              onValueChange={setQuery}
              placeholder="Tool, agent or project"
              aria-label="Search remembered rules"
              data-rule-search=""
            />
          }
        />
      ) : null}
      {searching && projects.length === 0 ? (
        <ItemRow
          label="No rule matches."
          detail={`Nothing remembered for “${query.trim()}”.`}
        />
      ) : null}
      {projects.map((project, index) => {
        const shown = !folds || searching || open.has(project.workspace)
        const count = `${project.rules.length} ${project.rules.length === 1 ? "rule" : "rules"}`
        return (
          <RuleProject
            key={project.workspace}
            project={project}
            first={index === 0}
            folds={folds}
            shown={shown}
            count={count}
            searching={searching}
            revoking={revoking}
            onToggle={() => toggle(project.workspace)}
            onRevoke={onRevoke}
          />
        )
      })}
    </>
  )
}

function RuleProject({
  project,
  first,
  folds,
  shown,
  count,
  searching,
  revoking,
  onToggle,
  onRevoke,
}: {
  project: Project
  first: boolean
  folds: boolean
  shown: boolean
  count: string
  searching: boolean
  revoking: boolean
  onToggle: () => void
  onRevoke: (ruleId: string) => void
}) {
  return (
    <>
      {folds ? (
        <ItemRow
          setting={first ? "remembered-rules" : undefined}
          label={projectName(project.workspace)}
          detail={`${count} · ${project.workspace}`}
          data-rule-project={project.workspace}
          control={
            searching ? undefined : (
              <button
                type="button"
                aria-expanded={shown}
                aria-label={`${shown ? "Hide" : "Show"} ${count} in ${projectName(project.workspace)}`}
                onClick={onToggle}
              >
                {shown ? "Hide" : "Show"}
              </button>
            )
          }
        />
      ) : null}
      {shown
        ? project.rules.map((rule, index) => (
            <ItemRow
              key={rule.id}
              setting={!folds && first && index === 0 ? "remembered-rules" : undefined}
              className={folds ? "settings-rule-in-project" : undefined}
              label={ruleLabel(rule)}
              detail={folds ? rule.source : `${rule.source} · ${rule.workspace}`}
              data-rule={rule.id}
              control={
                <button
                  type="button"
                  disabled={revoking}
                  onClick={() => onRevoke(rule.id)}
                >
                  Revoke
                </button>
              }
            />
          ))
        : null}
    </>
  )
}

type AppsPhase =
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly servers: readonly string[]
      readonly untrusting?: string
      readonly failed?: boolean
    }

/**
 * Settings › Privacy & Permissions › Access: the apps answered "Always allow
 * this app", whose messages go as the person without a review. Revoke one
 * and its next message is asked about again. Trust is kept by the gateway
 * for every chat, so the list is read again when the window comes back.
 */
export function TrustedAppRows() {
  const apps = useContext(PermissionRulesContext)?.trustedApps
  const [phase, setPhase] = useState<AppsPhase>({ kind: "reading" })
  const busy = useRef(false)
  // Each read of `apps` is one generation: an answer from an earlier one,
  // or one landing after this group left the page, is dropped.
  const generation = useRef(0)
  useEffect(() => {
    if (!apps) return
    let current = true
    const generations = generation
    generations.current++
    const read = () => {
      if (busy.current) return
      void apps.read().then((outcome) => {
        if (!current || busy.current) return
        setPhase(
          outcome.ok ? { kind: "ready", servers: outcome.servers } : { kind: "unread" },
        )
      })
    }
    setPhase({ kind: "reading" })
    read()
    const onVisible = () => {
      if (document.visibilityState === "visible") read()
    }
    window.addEventListener("focus", read)
    document.addEventListener("visibilitychange", onVisible)
    return () => {
      current = false
      busy.current = false
      generations.current++
      window.removeEventListener("focus", read)
      document.removeEventListener("visibilitychange", onVisible)
    }
  }, [apps])
  if (!apps) return null
  const untrust = (server: string) => {
    if (phase.kind !== "ready" || busy.current) return
    busy.current = true
    const asked = generation.current
    const before = phase.servers
    setPhase({ kind: "ready", servers: before, untrusting: server })
    const settle = (servers: readonly string[], failed: boolean) => {
      if (generation.current !== asked) return
      busy.current = false
      setPhase({ kind: "ready", servers, failed })
    }
    void apps
      .untrust(server, crypto.randomUUID())
      .then(async (outcome) => {
        const again = await apps.read()
        settle(
          again.ok
            ? again.servers
            : before.filter((each) => each !== server || !outcome.ok),
          !outcome.ok,
        )
      })
      .catch(() => settle(before, true))
  }
  const entry = setting("trusted-apps")
  return (
    <Group title={entry.label} note={entry.detail}>
      {phase.kind === "reading" || phase.kind === "unread" ? (
        <ItemRow
          setting="trusted-apps"
          label={entry.label}
          detail={
            phase.kind === "reading"
              ? "Reading apps."
              : "Could not read the apps you allow."
          }
          unavailable
        />
      ) : null}
      {phase.kind === "ready" && phase.failed ? (
        <ItemRow label="Could not revoke that app." detail="Try again in a moment." />
      ) : null}
      {phase.kind === "ready" && phase.servers.length === 0 ? (
        <ItemRow
          setting="trusted-apps"
          label={entry.label}
          detail="Apps you answer Always allow appear here."
        />
      ) : null}
      {phase.kind === "ready"
        ? phase.servers.map((server, index) => (
            <ItemRow
              key={server}
              setting={index === 0 ? "trusted-apps" : undefined}
              label={server}
              detail="Sends messages as you without asking."
              data-trusted-app={server}
              control={
                <button
                  type="button"
                  disabled={phase.untrusting !== undefined}
                  aria-label={`Revoke ${server}`}
                  onClick={() => untrust(server)}
                >
                  Revoke
                </button>
              }
            />
          ))
        : null}
    </Group>
  )
}
