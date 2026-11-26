import { Input } from "@nessa-ui/react/input"
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type {
  TerminalResumeRule,
  TerminalResumeSettings,
  TerminalResumeState,
} from "@nessa/client"
import type { TerminalResumeGateway } from "../adapters/terminal-resume-gateway"
import { setting } from "../model/settings-catalogue"
import {
  describeRule,
  draftRule,
  refusalMessage,
  RuleRefused,
  savedProgramDetail,
  type RuleDraft,
} from "../model/terminal-resume"
import { Choice, Group, ItemRow, RowAction, Toggle } from "./settings-controls"

/**
 * Settings › Terminal › Resume: whether programs in terminals come back
 * after Nessa restarts (OSC 7577), which programs may, and the programs kept
 * now. The gateway keeps all of it and does the resuming; this only reads
 * and changes it. With no gateway the rows rest.
 *
 * ```text
 * switch ── set {enabled, rules} ──▶ gateway (settings.json) ──▶ token in new terminals
 * rules  ── set {enabled, rules} ──▶ gateway ──▶ decides at the next start
 * kept   ◀── read / forget ─────── gateway (one file per terminal)
 * ```
 */
const TerminalResumeContext = createContext<TerminalResumeGateway | undefined>(undefined)

export function TerminalResumeProvider({
  gateway,
  children,
}: {
  gateway: TerminalResumeGateway | undefined
  children: ReactNode
}) {
  return (
    <TerminalResumeContext.Provider value={gateway}>
      {children}
    </TerminalResumeContext.Provider>
  )
}

type Phase =
  | { readonly kind: "reading" }
  | { readonly kind: "unread" }
  | {
      readonly kind: "ready"
      readonly state: TerminalResumeState
      readonly saving: boolean
    }

const actions = [
  { id: "allow", label: "Allow" },
  { id: "never", label: "Never" },
] as const

const emptyDraft: RuleDraft = { program: "", folder: "", args: "", action: "allow" }

export function TerminalResumeRows() {
  const gateway = useContext(TerminalResumeContext)
  const [phase, setPhase] = useState<Phase>({ kind: "reading" })
  const [failed, setFailed] = useState(false)
  const [retry, setRetry] = useState(0)
  const [draft, setDraft] = useState<RuleDraft>(emptyDraft)
  const [problem, setProblem] = useState("")
  const problemId = useId()
  // Only the newest read or change may show what it got.
  const revision = useRef(0)
  useEffect(() => {
    if (!gateway) return
    const current = ++revision.current
    setPhase({ kind: "reading" })
    gateway.read().then(
      (state) => {
        if (current === revision.current)
          setPhase({ kind: "ready", state, saving: false })
      },
      () => {
        if (current === revision.current) setPhase({ kind: "unread" })
      },
    )
    return () => {
      revision.current += 1
    }
  }, [gateway, retry])

  const change = (
    send: (gateway: TerminalResumeGateway) => Promise<TerminalResumeState>,
    shown: TerminalResumeState,
    kept?: () => void,
  ) => {
    if (!gateway || phase.kind !== "ready" || phase.saving) return
    const before = phase.state
    const current = ++revision.current
    setFailed(false)
    setPhase({ kind: "ready", state: shown, saving: true })
    send(gateway).then(
      (state) => {
        if (current !== revision.current) return
        setPhase({ kind: "ready", state, saving: false })
        kept?.()
      },
      (cause: unknown) => {
        if (current !== revision.current) return
        if (cause instanceof RuleRefused) {
          // Refused, so nothing changed: show what was there, and why.
          setPhase({ kind: "ready", state: before, saving: false })
          setProblem(refusalMessage(cause.part))
          return
        }
        // A lost answer may still have been kept: read what is in effect.
        setFailed(true)
        setRetry((value) => value + 1)
      },
    )
  }
  const save = (settings: TerminalResumeSettings, kept?: () => void) => {
    if (phase.kind !== "ready") return
    change((each) => each.set(settings), { ...phase.state, settings }, kept)
  }
  const forget = (terminalId: string) => {
    if (phase.kind !== "ready") return
    change((each) => each.forget(terminalId), {
      ...phase.state,
      saved: phase.state.saved.filter((saved) => saved.terminalId !== terminalId),
    })
  }

  const ready = !!gateway && phase.kind === "ready"
  const state = phase.kind === "ready" ? phase.state : undefined
  const busy = phase.kind === "ready" && phase.saving
  const settings = state?.settings
  const on = settings?.enabled ?? false
  const rules = settings?.rules ?? []
  const status = !gateway
    ? "Connect a gateway"
    : phase.kind === "reading"
      ? "Loading…"
      : undefined
  const toggle = setting("terminal-resume")
  const programs = setting("terminal-resume-programs")
  const kept = setting("terminal-resume-kept")

  const add = () => {
    if (!settings) return
    setProblem("")
    // The draft stays until the gateway keeps the rule, so a refusal can be
    // fixed in place.
    save({ ...settings, rules: [...settings.rules, draftRule(draft)] }, () =>
      setDraft(emptyDraft),
    )
  }
  const remove = (at: number) => {
    if (!settings) return
    save({ ...settings, rules: settings.rules.filter((_, index) => index !== at) })
  }

  return (
    <>
      <Group>
        <ItemRow
          label={toggle.label}
          detail={status ?? toggle.detail}
          setting="terminal-resume"
          unavailable={!ready}
          data-terminal-resume={settings ? String(settings.enabled) : undefined}
          control={
            <Toggle
              checked={on}
              disabled={busy}
              onChange={(next) => settings && save({ ...settings, enabled: next })}
            />
          }
        />
      </Group>
      <Group
        title={programs.label}
        note="A program comes back only when an Allow rule matches it and no Never rule does. A name matches the program's file name; a path matches only that file."
      >
        {rules.length === 0 && (
          <ItemRow
            label="No programs yet"
            detail={
              on
                ? "Nothing comes back until you add one."
                : "Add the programs to bring back once resume is on."
            }
            unavailable={!ready}
          />
        )}
        {rules.map((rule: TerminalResumeRule, at) => (
          <ItemRow
            // Rules have no ids; their place and text say which one this is.
            key={`${at}:${rule.action}:${rule.program}:${rule.folder ?? ""}:${(rule.args ?? []).join(" ")}`}
            label={<code className="settings-terminal-program">{rule.program}</code>}
            detail={describeRule(rule)}
            unavailable={!ready}
            data-terminal-resume-rule={rule.program}
            data-terminal-resume-action={rule.action}
            control={
              <RowAction disabled={busy} onClick={() => remove(at)}>
                Remove
              </RowAction>
            }
          />
        ))}
        <ItemRow
          label="Add a program"
          setting="terminal-resume-programs"
          unavailable={!ready}
          describedBy={problem ? problemId : undefined}
          data-terminal-resume-add=""
          control={
            <RowAction disabled={busy} onClick={add} data-terminal-resume-add-button="">
              Add
            </RowAction>
          }
        >
          <span className="settings-terminal-rule-form">
            <Input
              className="settings-terminal-rule-field"
              type="text"
              aria-label="Program"
              placeholder="claude, nvim, or /path/to/program"
              aria-invalid={problem ? true : undefined}
              aria-describedby={problem ? problemId : undefined}
              data-terminal-resume-program=""
              disabled={!ready}
              value={draft.program}
              onChange={(event) => {
                setDraft({ ...draft, program: event.target.value })
                if (problem) setProblem("")
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return
                event.preventDefault()
                add()
              }}
            />
            <Input
              className="settings-terminal-rule-field"
              type="text"
              aria-label="Only in folder"
              placeholder="Any folder"
              data-terminal-resume-folder=""
              disabled={!ready}
              value={draft.folder}
              onChange={(event) => setDraft({ ...draft, folder: event.target.value })}
            />
            <Input
              className="settings-terminal-rule-field"
              type="text"
              aria-label="Only with arguments"
              placeholder="Any arguments"
              data-terminal-resume-args=""
              disabled={!ready}
              value={draft.args}
              onChange={(event) => setDraft({ ...draft, args: event.target.value })}
            />
            <Choice
              label="Rule"
              value={draft.action}
              options={actions}
              data-terminal-resume-choice=""
              onChange={(action) => setDraft({ ...draft, action })}
            />
          </span>
        </ItemRow>
        {problem && (
          <p id={problemId} role="alert" className="settings-boat-note">
            {problem}
          </p>
        )}
      </Group>
      <Group title={kept.label} note={kept.detail}>
        {(state?.saved.length ?? 0) === 0 && (
          <ItemRow
            label="Nothing kept"
            detail="A program shows here once it asks to come back."
            unavailable={!ready}
          />
        )}
        {state?.saved.map((saved) => (
          <ItemRow
            key={saved.terminalId}
            label={saved.title}
            detail={savedProgramDetail(saved)}
            unavailable={!ready}
            data-terminal-resume-kept={saved.terminalId}
            data-terminal-resume-comes-back={String(saved.comesBack)}
            control={
              <RowAction disabled={busy} onClick={() => forget(saved.terminalId)}>
                Forget
              </RowAction>
            }
          />
        ))}
      </Group>
      {(phase.kind === "unread" || failed) && gateway && (
        <p role="alert" className="settings-boat-note">
          {failed ? "Couldn’t confirm the change." : "Couldn’t read this."}{" "}
          {phase.kind === "unread" && (
            <button
              className="settings-button"
              type="button"
              onClick={() => setRetry((value) => value + 1)}
            >
              Reload
            </button>
          )}
        </p>
      )}
    </>
  )
}
