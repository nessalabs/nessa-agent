import { installationOutcomeText } from "./agent-downloads"
import type {
  AgentInstallations,
  InstallationOffer,
} from "../application/agent-installations"
import { useAgentDownloads } from "./use-agent-downloads"
import * as React from "react"
import type { AgentApiKeySink } from "../application/ports"
import { AgentApiKeyAuditWarning, useAgentApiKeySave } from "./agent-api-key-save"
import { AgentApiKeyForm } from "./agent-api-key-form"
import {
  Check,
  CircleAlert,
  CircleHelp,
  Clock,
  Download,
  KeyRound,
  LoaderCircle,
  Cpu,
  Settings,
  Unplug,
} from "lucide-react"
import { AgentMark } from "./agent-mark"
import { Keycaps } from "./keycaps"
import { useHeldKeys } from "./use-held-keys"
import type { RuntimeCopy } from "../../host"
import type { GatewayStartupStatus } from "../../startup/application/gateway-startup"
import { COULD_NOT_START } from "../../startup/application/copy"
import type { ShortcutPlatform } from "../model/shortcut-display"
import { AgentBackdrop } from "@nessa-ui/react/agent-card"
import { AgentFigure } from "@nessa-ui/react/agent-figure/agent-figure"
import { Button } from "@nessa-ui/react/button"
import { StepDots } from "@nessa-ui/react/step-dots"
import { TypeIn } from "@nessa-ui/react/type-in"
import {
  SETUP_AGENTS,
  agentReadiness,
  isChoosable,
  type AgentId,
  type AgentReadiness,
  type AgentReadinessFailure,
  type OnboardingState,
} from "../model/onboarding"
import type { FigureId } from "../model/opening"
import type { LingerShown, LingerView } from "../model/linger"

/** The id the setup window's `aria-labelledby` points at. Every step titles the
 * dialog with its own heading, so there is exactly one of these on screen. */
export const SETUP_HEADING_ID = "nessa-setup-heading"

/** Every button in a setup pane: one height, one width, one shape. */
const PANE_BUTTON = "h-11 w-full rounded-full nessa-text-3 font-medium"

/** A step's way on: the pane's one dark pill, as wide as its words. */
const PANE_PILL = "h-11 self-start rounded-full px-6 nessa-text-3 font-medium"

/** One row of the agent picker, on the pane's light glass. */
const OPTION =
  "flex w-full items-center gap-3 rounded-xl border border-border bg-card/55 px-3 py-2.5 text-left transition-[background-color,border-color,box-shadow] outline-none hover:bg-card/85 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 aria-pressed:border-foreground/55 aria-pressed:bg-card aria-pressed:shadow-md"

/** A step's heading in the pane. */
const PANE_HEADING = "nessa-setup-display font-semibold text-foreground"

/** The steps the dots count. Linger is the last step's own question, so it
 * keeps the last dot rather than adding one only some systems ever see. */
const DOTTED_STEPS = ["welcome", "agent", "summon"] as const

function dotFor(step: OnboardingState["step"]): number {
  if (step === "linger" || step === "done") return DOTTED_STEPS.length - 1
  return DOTTED_STEPS.indexOf(step)
}

/**
 * What the summon step says, which is a different thing at each press.
 *
 * The same keys do both jobs, so the copy has to name the one that has not
 * been shown yet: first that it summons, then that the very same press puts it
 * away, then that the pair of them is the whole trick.
 *
 * What the panel is doing comes from the host, not from a press count, so this
 * cannot end up telling someone to hide a panel that is already hidden.
 */
function summonHeading(state: OnboardingState, configured: boolean) {
  if (!configured) return "No summon shortcut is set"
  if (state.summonTaught) return "That’s all it takes"
  if (state.summon === "shown") return "Press it again to hide"
  // No press has been reported yet. The panel could already be showing (it
  // is, at this point, on Linux) or not — this step does not know, and
  // "Summon it from anywhere" would be wrong for whichever it turns out not
  // to be. Neutral copy is correct regardless of where the panel started.
  if (state.summon === undefined) return "Try it now"
  return "Summon it from anywhere"
}

/**
 * One step of setup: its words and controls, on the pane.
 *
 * The pane is the step's focus target and its scroller. Everything in it is in
 * normal flow, centred down the pane, and the pane scrolls: a button clipped
 * off the window at a large text size is a setup nobody can finish.
 */
const SetupPane = React.forwardRef<
  HTMLDivElement,
  { step: OnboardingState["step"]; children: React.ReactNode }
>(function SetupPane({ step, children }, ref) {
  return (
    <div ref={ref} tabIndex={-1} className="nessa-setup-pane nessa-setup-light">
      <StepDots
        className="nessa-setup-dots"
        count={DOTTED_STEPS.length}
        current={dotFor(step)}
      />
      <div className="my-auto flex flex-col gap-5 pt-4">{children}</div>
    </div>
  )
})

/**
 * What an agent that cannot be picked is waiting on: a mark, and the words it
 * stands for.
 *
 * The words are the mark's name — announced, and shown beside it when the row
 * is hovered or focused — rather than printed on every row. A column of sentences down the list is noise, and
 * most of these differ only in which small fix they ask for, which a glyph says
 * at a glance.
 *
 * A gateway that failed to start is not listed here at all. It is not a fact
 * about any one agent, so the picker gives way to a screen of its own.
 *
 * "Nothing answered" is not a fact about the agent and does not read as one.
 * Collapsing an unreachable gateway into the same words as an agent Nessa does
 * not support yet told someone their setup was fine when it was not.
 */
type ReadinessNote = { text: string; Icon: typeof Check; spin?: boolean }

function readinessNote(
  readiness: AgentReadiness,
  failure: AgentReadinessFailure | undefined,
  startup: GatewayStartupStatus,
): ReadinessNote | undefined {
  if (startup.state === "starting") {
    return { text: "Starting Nessa…", Icon: LoaderCircle, spin: true }
  }
  if (readiness === "ready") return undefined
  if (readiness === "not-supported") return { text: "Coming soon", Icon: Clock }
  if (readiness === "needs-authentication") {
    return { text: "Needs sign-in", Icon: KeyRound }
  }
  if (readiness === "not-installed") return { text: "Not installed", Icon: Download }
  // Not "not installed": this Nessa was not set up to run it, and the agent may
  // be sitting on the machine already. Telling someone to install what they
  // have is advice that cannot work however many times they take it.
  if (readiness === "not-configured") {
    return { text: "Not set up here", Icon: Settings }
  }
  // Each failure keeps a mark of its own, so which one it is can be told at a
  // glance and is not left to the words alone.
  if (failure === "unreachable") return { text: "Can’t reach Nessa", Icon: Unplug }
  if (failure === "unreadable") return { text: "Unexpected answer", Icon: CircleHelp }
  return { text: "Checking…", Icon: LoaderCircle, spin: true }
}

/** How far the private runtime copy has got, as a percent of bytes when the
 * host reported a size, otherwise of files. An empty total stays at zero.
 * While the host is still copying, 100 is held back: rounding a nearly
 * finished byte count, or a byte total that ignores a zero-length file still
 * syncing, would otherwise read as done. */
function runtimeCopyValue(progress: RuntimeCopy): number {
  const total = progress.bytesTotal > 0 ? progress.bytesTotal : progress.filesTotal
  const completed =
    progress.bytesTotal > 0 ? progress.bytesCompleted : progress.filesCompleted
  if (total === 0) return 0
  const percent = Math.min(100, Math.round((completed / total) * 100))
  if (progress.state === "copying" && percent === 100) return 99
  return percent
}

/** Shown while files are still finishing `sync_all`. Settled and a missing
 * report draw nothing. An agent's own readiness decides whether it can be picked. */
function RuntimeCopyBar({ progress }: { progress?: RuntimeCopy }) {
  if (progress?.state !== "copying") return null
  const value = runtimeCopyValue(progress)
  return (
    <progress
      aria-label="Preparing the private runtime"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value}
      max={100}
      value={value}
      className="h-1.5 w-full"
    />
  )
}

/** One listed agent: its mark, its name, and what it is waiting on. An agent
 * that cannot run is shown with the reason rather than hidden — a missing
 * Claude is a question, and a Claude that says it needs signing in is an
 * answer.
 *
 * Which is why the unpickable ones are `aria-disabled` rather than `disabled`.
 * A disabled button is out of the tab order, so the reason it carries — the
 * entire point of listing it — was reachable by a screen reader in browse mode
 * and by nobody else. It stays focusable and does nothing when pressed. */
function AgentOption({
  id,
  name,
  readiness,
  failure,
  startup,
  selected,
  onSelect,
  takesFocus,
  onFocusTaken,
}: {
  id: AgentId
  name: string
  readiness: AgentReadiness
  failure: AgentReadinessFailure | undefined
  startup: GatewayStartupStatus
  selected: boolean
  onSelect: (id: AgentId) => void
  /** Take the focus when shown: this row has replaced the download card that
   * held it. */
  takesFocus?: boolean
  onFocusTaken?: () => void
}) {
  const note = readinessNote(readiness, failure, startup)
  const row = React.useRef<HTMLButtonElement>(null)
  React.useEffect(() => {
    if (!takesFocus) return
    row.current?.focus()
    onFocusTaken?.()
  }, [takesFocus, onFocusTaken])
  return (
    <button
      ref={row}
      type="button"
      // Plain toggles rather than a radio group. The group announced the
      // arrow-key pattern a radio group promises and implemented none of it:
      // arrows did nothing and every option was its own tab stop. Saying what
      // the markup actually does is the smaller of the two honest changes.
      aria-pressed={selected}
      aria-disabled={note ? true : undefined}
      // The note is only a mark on screen, so the option is named outright with
      // the words the mark stands for.
      aria-label={note ? `${name}, ${note.text.toLowerCase()}` : name}
      onClick={() => {
        if (note) return
        onSelect(id)
      }}
      className={`group ${OPTION} aria-disabled:cursor-default aria-disabled:opacity-60`}
    >
      <span className="flex size-8 shrink-0 items-center justify-center text-foreground">
        <AgentMark id={id} name={name} />
      </span>
      <span className="nessa-text-4 font-medium text-foreground">{name}</span>
      {note ? (
        // The words come up beside the mark whenever the row is pointed at or
        // focused — by a click or by a key — so the reason is on screen for
        // anyone who asks for it, not only for a pointer that can hover or a
        // screen reader that reads the name. Hidden from assistive technology
        // because the option's name already says it.
        <span
          aria-hidden="true"
          className="ml-auto hidden nessa-text-2 whitespace-nowrap text-muted-foreground group-hover:inline group-focus:inline"
        >
          {note.text}
        </span>
      ) : null}
      <span
        className={`${note ? "group-hover:ml-0 group-focus:ml-0 " : ""}ml-auto flex size-5 shrink-0 items-center justify-center`}
      >
        {note ? (
          <note.Icon
            aria-hidden
            className={`size-4 text-muted-foreground${note.spin ? " animate-spin" : ""}`}
          />
        ) : (
          <span className="flex size-5 shrink-0 items-center justify-center rounded-full border border-border">
            {selected ? <Check aria-hidden className="size-3.5" /> : null}
          </span>
        )}
      </span>
    </button>
  )
}

/** Sentences for one host tag. `enabled` is the only claim of logged-out operation. */
function lingerCopy(shown: LingerShown): { heading: string; detail: string } {
  switch (shown) {
    case "offer":
      return {
        heading: "Keep Nessa running when you log out?",
        detail: "The gateway stops when you log out unless this account lingers.",
      }
    case "enabled":
      return {
        heading: "Nessa keeps running when you log out",
        detail: "This account lingers, so the gateway stays running after you log out.",
      }
    case "refused":
      return {
        heading: "Nessa runs while you are signed in",
        detail: "Staying on after logout was not turned on.",
      }
    case "failed":
      return {
        heading: "Nessa runs while you are signed in",
        detail: "Nessa could not turn that on.",
      }
    case "unsupported":
      return {
        heading: "Nessa cannot stay running after you log out",
        detail: "This system has no login service to ask.",
      }
    case "not-applicable":
      return {
        heading: "Nessa could not confirm that",
        detail: "Login did not say whether this account lingers.",
      }
  }
}

/** The linger step. What it says is the host's `shown` tag. */
export const LingerStep = React.forwardRef<
  HTMLDivElement,
  {
    view: LingerView | undefined
    pending: boolean
    onAccept: () => void
    onFinish: () => void
  }
>(function LingerStep({ view, pending, onAccept, onFinish }, ref) {
  const shown: LingerShown = view?.shown ?? "failed"
  const claim = shown === "enabled" && !pending ? "yes" : "no"
  const { heading, detail } = lingerCopy(shown)
  const manual = shown === "refused" || shown === "failed"
  return (
    <SetupPane ref={ref} step="linger">
      <div className="flex flex-col gap-3" data-linger={shown} data-linger-claim={claim}>
        <h1 id={SETUP_HEADING_ID} className={PANE_HEADING}>
          {heading}
        </h1>
        <p
          role="status"
          aria-live="polite"
          className="nessa-text-3 text-muted-foreground"
        >
          {detail}
        </p>
        {manual ? (
          <p className="nessa-text-3 text-muted-foreground">
            You can run <code>loginctl enable-linger</code>.
          </p>
        ) : null}
      </div>
      {shown === "offer" ? (
        <>
          <Button size="lg" className={PANE_BUTTON} disabled={pending} onClick={onAccept}>
            Keep it running
          </Button>
          <Button size="lg" className={PANE_BUTTON} disabled={pending} onClick={onFinish}>
            Only while I’m signed in
          </Button>
        </>
      ) : null}
      {shown === "refused" || shown === "failed" ? (
        <>
          <Button size="lg" className={PANE_BUTTON} disabled={pending} onClick={onAccept}>
            Try again
          </Button>
          <Button size="lg" className={PANE_BUTTON} disabled={pending} onClick={onFinish}>
            Start using Nessa
          </Button>
        </>
      ) : null}
      {shown === "enabled" || shown === "unsupported" || shown === "not-applicable" ? (
        <Button size="lg" className={PANE_BUTTON} disabled={pending} onClick={onFinish}>
          Start using Nessa
        </Button>
      ) : null}
    </SetupPane>
  )
})

/**
 * Whether the agent picker is what setup is showing. A gateway that did not
 * come up replaces it with a retry, so the picture beside it has no agents to
 * stand there either.
 */
export function showsPicker(
  state: OnboardingState,
  gatewayStartup: GatewayStartupStatus,
): boolean {
  return (
    state.step === "agent" &&
    gatewayStartup.state !== "failed" &&
    gatewayStartup.state !== "unavailable"
  )
}

/** The figure that stands for each agent: Claude's is Claude Code's mascot. */
const FIGURE_OF = Object.freeze({
  claude: "clawd",
  codex: "codex",
  local: "local",
} as const satisfies Record<FigureId, string>)

const SHOWCASE: readonly FigureId[] = Object.freeze(["claude", "codex", "local"])

/** How long a figure plays once its agent is chosen, before it rests again. */
const CHOSEN_PLAY_MS = 2_400

/**
 * The picture beside the picker, taken over by one agent: the chosen one, else
 * Claude. It follows the choice and nothing else, so pointing along the rows
 * leaves it alone and a click is what turns it. The whole picture wears that
 * agent's card background, which fades across when the choice changes, with
 * its figure standing on it; a new choice plays the figure for a moment.
 * Every background is drawn and only the shown one is opaque, so a change is
 * a crossfade rather than a cut. The figure is keyed by agent and fades in
 * fresh. Decorative: the rows say all of it, so nothing here is named.
 */
export function AgentShowcase({ chosen }: { chosen: AgentId | undefined }) {
  const shown: FigureId = chosen === "codex" ? "codex" : "claude"
  const lively = usePlayOnChange(shown)
  return (
    <>
      {SHOWCASE.map((id) => (
        <AgentBackdrop
          key={id}
          agent={FIGURE_OF[id]}
          className="nessa-setup-backdrop"
          data-shown={id === shown ? "" : undefined}
        />
      ))}
      <AgentFigure
        key={shown}
        agent={FIGURE_OF[shown]}
        lively={lively}
        className="nessa-setup-figure"
      />
    </>
  )
}

/** True for a moment after `value` changes, never for the value it started
 * with: the figure on screen when the picker opens has not just been chosen. */
function usePlayOnChange<T>(value: T): boolean {
  const last = React.useRef(value)
  const [playing, setPlaying] = React.useState(false)
  React.useEffect(() => {
    if (value === last.current) return
    last.current = value
    setPlaying(true)
    const timer = setTimeout(() => setPlaying(false), CHOSEN_PLAY_MS)
    return () => clearTimeout(timer)
  }, [value])
  return playing
}

type Downloads = ReturnType<typeof useAgentDownloads> & {
  /** An install has finished and the readiness check that follows it has not
   * yet come back, so the agent is held as an installed card until it does. */
  installHeld: boolean
}

/** The picker's downloads, read only where this surface can install. A hook
 * cannot be called on one render and not the next, so the read lives in a
 * component of its own that is there or is not. */
function WithDownloads({
  source,
  onChanged,
  checking,
  children,
}: {
  source: AgentInstallations | undefined
  onChanged: () => void
  checking: boolean
  children: (downloads: Downloads | undefined) => React.ReactNode
}) {
  if (!source) return <>{children(undefined)}</>
  return (
    <ReadDownloads source={source} onChanged={onChanged} checking={checking}>
      {children}
    </ReadDownloads>
  )
}

function ReadDownloads({
  source,
  onChanged,
  checking,
  children,
}: {
  source: AgentInstallations
  onChanged: () => void
  checking: boolean
  children: (downloads: Downloads) => React.ReactNode
}) {
  const downloads = useAgentDownloads(source, onChanged)
  return (
    <>
      {children({
        ...downloads,
        installHeld: useInstallHeld(downloads.outcome, checking),
      })}
    </>
  )
}

/** The longest an install waits on its readiness check before the card lets
 * go: the check may never start (a gateway that is not ready asks nothing). */
const INSTALL_HOLD_MS = 5_000

/**
 * Whether an install is waiting on the readiness check it starts. The check
 * is asked for once the downloads have been read again, a moment after the
 * install reports, so "not checking" alone would let the row show through for
 * that moment. Held until a check has been seen running and has come back, or
 * for `INSTALL_HOLD_MS` at most, so a check that never runs cannot leave the
 * card standing for good.
 */
function useInstallHeld(outcome: Downloads["outcome"], checking: boolean): boolean {
  const installed = outcome?.status === "installed" ? outcome : undefined
  // The install whose wait is over: its check came back, or it waited long
  // enough. Keyed by the outcome, which each install makes afresh.
  const [released, setReleased] = React.useState<Downloads["outcome"]>()
  const [ran, setRan] = React.useState<Downloads["outcome"]>()
  React.useEffect(() => {
    if (!installed || released === installed) return
    if (checking) setRan(installed)
    else if (ran === installed) setReleased(installed)
  }, [installed, checking, ran, released])
  React.useEffect(() => {
    if (!installed) return
    const timer = setTimeout(() => setReleased(installed), INSTALL_HOLD_MS)
    return () => clearTimeout(timer)
  }, [installed])
  return installed !== undefined && released !== installed
}

/** What an agent's download card is doing, when its place in the picker is a
 * card rather than a row. */
type CardState =
  /** Offered: one Download button with the size. */
  | { kind: "offer"; offer: InstallationOffer }
  /** Being fetched and verified. */
  | { kind: "fetching" }
  /** Installed, until the readiness check that follows turns it into the row. */
  | { kind: "installed" }
  /** Downloads are still being read: the card holds the agent's place, so a
   * row does not stand there only to turn into a card a moment later. */
  | { kind: "reading" }
  /** Nothing can be offered until downloads are read again: they could not
   * be read, or an attempt was lost or met another one still running. */
  | { kind: "check" }

/**
 * Whether an agent's place in the picker is its download card rather than its
 * row, and what the card is doing. Only an agent this machine does not have
 * gets one. A fresh read of the downloads always wins over what the last
 * attempt said, so an agent that read can no longer offer goes back to its row
 * rather than standing behind a card for good. An install is held as a card
 * only until the readiness check after it is done: an agent that check still
 * cannot find goes back to its row and its reason.
 */
function cardState(
  id: AgentId,
  readiness: AgentReadiness,
  downloads: Downloads | undefined,
): CardState | undefined {
  if (readiness !== "not-installed" || !downloads) return undefined
  if (downloads.pending === id) return { kind: "fetching" }
  if (downloads.attempted === id && downloads.installHeld) return { kind: "installed" }
  const offer = downloads.offers?.find((each) => each.agent === id)
  if (downloads.unavailable) return { kind: "check" }
  if (!downloads.offers)
    return downloads.attempted === id && downloads.outcome
      ? { kind: "check" }
      : { kind: "reading" }
  return offer && !offer.installed ? { kind: "offer", offer } : undefined
}

/** Why a card's last download did not install, in the card's own words: its
 * one way to read the downloads again is its Check again button. */
function cardFailureText(downloads: Downloads, id: AgentId): string | undefined {
  if (downloads.attempted !== id || downloads.outcome?.status !== "failed")
    return downloads.unavailable && downloads.pending === undefined
      ? "Downloads could not be checked."
      : undefined
  switch (downloads.outcome.reason) {
    case "busy":
      return "Another download is running. Check again shortly."
    case "not-confirmed":
      return "Not confirmed. It may still be running; check again before trying again."
    default:
      return installationOutcomeText(downloads.outcome)
  }
}

/**
 * An agent that is not on this machine yet, in the place its row would take:
 * its mark, its name, and one button. Nothing to pick until it is here, so it
 * is a card rather than a toggle; once installed, a fresh readiness check
 * turns it into the row.
 *
 * Words only when there is something to act on — a download that failed says
 * why, under the name. The size rides on the button, which is all a person
 * needs to decide.
 *
 * The button stays the same element through a download, held with
 * `aria-disabled` rather than `disabled`, so the focus a key press put on it
 * stays there while it turns into the download's spinner; when the card
 * becomes the row, the row takes that focus.
 */
function AgentDownloadCard({
  id,
  name,
  card,
  downloads,
  onFocusChange,
}: {
  id: AgentId
  name: string
  card: CardState
  downloads: Downloads
  /** Told when the card's button takes or loses the focus, so the row the
   * card becomes can take it over. */
  onFocusChange: (id: AgentId | undefined) => void
}) {
  const failed = cardFailureText(downloads, id)
  const busy = downloads.pending !== undefined
  const megabytes =
    card.kind === "offer" ? Math.ceil(card.offer.archiveBytes / 1_000_000) : undefined
  const label =
    card.kind === "fetching"
      ? `Downloading and verifying ${name}`
      : card.kind === "installed"
        ? `${name} is installed`
        : card.kind === "reading"
          ? `Checking ${name} downloads`
          : card.kind === "offer"
            ? `Download ${name} · ${megabytes} MB`
            : "Check again"
  return (
    <div
      role="group"
      aria-label={`Download ${name}`}
      data-agent-download={id}
      data-download-state={card.kind}
      className="flex w-full items-center gap-3 rounded-xl border border-dashed border-border bg-card/35 px-3 py-2.5"
    >
      <span className="flex size-8 shrink-0 items-center justify-center text-foreground">
        <AgentMark id={id} name={name} />
      </span>
      <span className="flex min-w-0 flex-col">
        <span className="nessa-text-4 font-medium text-foreground">{name}</span>
        <span
          role="status"
          aria-live="polite"
          className="nessa-text-2 text-muted-foreground"
        >
          {failed}
        </span>
      </span>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="ml-auto shrink-0 rounded-full aria-disabled:opacity-60"
        aria-label={label}
        // Another agent's download holds this one too: the installer runs
        // one at a time.
        aria-disabled={
          card.kind === "installed" ||
          card.kind === "reading" ||
          (card.kind !== "check" && busy)
        }
        onFocus={() => onFocusChange(id)}
        onBlur={() => onFocusChange(undefined)}
        onClick={() => {
          if (card.kind === "check") downloads.refresh()
          else if (card.kind === "offer" && !busy) downloads.install(card.offer.agent)
        }}
      >
        {card.kind === "fetching" || card.kind === "reading" ? (
          <LoaderCircle aria-hidden className="motion-safe:animate-spin" />
        ) : card.kind === "installed" ? (
          <Check aria-hidden />
        ) : card.kind === "offer" ? (
          <>
            <Download aria-hidden />
            {megabytes} MB
          </>
        ) : (
          "Check again"
        )}
      </Button>
    </div>
  )
}

/** The Local AI row: listed so the picker says it is coming, and never
 * choosable. Named with what it is waiting on, like any agent that cannot be
 * picked, and still focusable so a keyboard reaches that reason. */
function LocalAiOption() {
  return (
    <button
      type="button"
      aria-disabled
      aria-label="Local AI, coming soon"
      className={`${OPTION} aria-disabled:cursor-default aria-disabled:opacity-60 aria-disabled:hover:bg-card/55`}
    >
      <span className="flex size-8 shrink-0 items-center justify-center text-foreground">
        <Cpu aria-hidden className="size-5" />
      </span>
      <span className="nessa-text-4 font-medium text-foreground">Local AI</span>
      <span
        aria-hidden="true"
        className="ml-auto nessa-text-2 whitespace-nowrap text-muted-foreground"
      >
        Coming soon
      </span>
    </button>
  )
}

/**
 * First-run setup, as the step on the pane and what the picture shows beside
 * it.
 *
 * This renders and dispatches; it holds no setup state and decides nothing.
 * Choosing an agent records the choice and nothing more — no credential is
 * collected and no provider is started — so the copy promises setup, not a
 * ready agent.
 */
export function Onboarding({
  state,
  open = true,
  apiKeys,
  installations,
  gatewayStartup,
  accelerator,
  onBegin,
  onChoose,
  onConfirm,
  onFinish,
  onAcceptLinger,
  lingerPending = false,
  onRecheck,
  onRetryGateway,
  checking = false,
  platform,
  runtimeCopy,
}: {
  state: OnboardingState
  /** Whether the window has opened. Until it has, the welcome waits to type
   * and nothing takes the focus. Defaults to true. */
  open?: boolean
  /** Secure-store effect injected by native setup composition. */
  apiKeys?: AgentApiKeySink
  installations?: AgentInstallations
  gatewayStartup: GatewayStartupStatus
  accelerator?: string
  /** The keyboard conventions this device writes shortcuts in. */
  platform: ShortcutPlatform
  onBegin: () => void
  onChoose: (id: AgentId) => void
  onConfirm: () => void
  onFinish: () => void
  /** Enable linger. Absent on a surface that never reaches the step. */
  onAcceptLinger?: () => void
  /** True while that enable has not returned. The buttons stop taking presses. */
  lingerPending?: boolean
  /** Ask the runtimes again, for whoever has just fixed what was wrong. */
  onRecheck: () => void
  /** Ask the native owner to reconcile the gateway again after a failure. */
  onRetryGateway: () => void
  /** True while an ask is in flight. The button that starts one says so and
   * stops taking presses, because each ask runs a real probe per agent and a
   * button that looks inert invites being pressed again. */
  checking?: boolean
  /** Private-runtime copy progress. Absent in a browser and after reporting
   * stops. An agent's own readiness decides whether it can be picked. */
  runtimeCopy?: RuntimeCopy
}) {
  const credentials = useAgentApiKeySave(apiKeys)
  // Called unconditionally, as a hook must be; it only listens on the step
  // that has keys to light.
  const held = useHeldKeys(state.step === "summon")
  // Whether the welcome has finished typing its name: the line under it and
  // its way on come in then. Back to the welcome types it again, so the rest
  // waits for it again rather than standing under half a name.
  const [typed, setTyped] = React.useState(false)
  React.useEffect(() => {
    if (state.step !== "welcome") setTyped(false)
  }, [state.step])

  // Each step is a different subtree, so a button that moves setup on takes its
  // own focus with it and leaves the caret on `<body>`. A keyboard user then
  // tabs from the top of the document every time and a screen reader is told
  // nothing at all. The new step takes the focus instead — once the window is
  // open, because a pane still hidden behind the glow cannot hold it.
  const step = React.useRef<HTMLDivElement>(null)
  // Which download card's button holds the focus, so the row it becomes can
  // take it: the button goes with the card, and the focus would go to the page.
  const [focusedCard, setFocusedCard] = React.useState<AgentId>()
  React.useEffect(() => {
    if (open) step.current?.focus()
  }, [state.step, open])

  if (state.step === "welcome") {
    return (
      <SetupPane ref={step} step={state.step}>
        <div className="flex flex-col gap-3" data-typed={typed ? "" : undefined}>
          <h1
            id={SETUP_HEADING_ID}
            aria-label="Welcome to Nessa"
            className={PANE_HEADING}
          >
            <TypeIn
              text="Welcome to Nessa"
              plain={11}
              play={open}
              delay={250}
              onTyped={() => setTyped(true)}
            />
          </h1>
          <p className="nessa-setup-after nessa-text-5 text-muted-foreground">
            Your personal sidekick
          </p>
          <Button
            size="lg"
            className={`nessa-setup-after mt-4 ${PANE_PILL}`}
            onClick={onBegin}
          >
            Get started
          </Button>
        </div>
      </SetupPane>
    )
  }

  if (state.step === "summon") {
    const heading = summonHeading(state, Boolean(accelerator))
    // The lesson is the point of the step, so the primary action waits for it:
    // offering it beforehand invites a click straight past the only thing the
    // step is here to teach, and the button arriving *because* the keys lit is
    // what says they worked. What must never wait for it is a way out. A
    // configuration with no accelerator has nothing to press, and a chord is
    // not something everyone can hold — either way the step was a dead end with
    // no exit but abandoning setup, which records nothing and starts over.
    const taught = state.summonTaught === true || !accelerator
    return (
      <SetupPane ref={step} step={state.step}>
        <h1 id={SETUP_HEADING_ID} className={`nessa-setup-arrive ${PANE_HEADING}`}>
          {heading}
        </h1>
        {/* The lesson's only report is that the heading changed, which is a
          change nothing announces. A press of a global shortcut is exactly
          the moment someone needs telling it registered. */}
        <p role="status" aria-live="polite" className="sr-only">
          {heading}
        </p>
        {accelerator ? (
          <Keycaps
            keys={accelerator}
            platform={platform}
            pressed={state.summon}
            held={held}
          />
        ) : null}
        {taught ? (
          <Button
            size="lg"
            className={`nessa-setup-arrive ${PANE_PILL}`}
            onClick={onFinish}
          >
            Start using Nessa
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="lg"
            onClick={onFinish}
            className={`nessa-setup-arrive ${PANE_PILL} -ml-3 text-muted-foreground`}
          >
            Skip this step
          </Button>
        )}
      </SetupPane>
    )
  }

  if (state.step === "linger") {
    return (
      <LingerStep
        ref={step}
        view={state.linger}
        pending={lingerPending}
        onAccept={onAcceptLinger ?? (() => {})}
        onFinish={onFinish}
      />
    )
  }

  // A gateway that did not come up is the only thing worth saying: no agent can
  // be checked, chosen or started until it does, so the list gives way to it
  // rather than printing one failure beside every agent. The host's own words
  // for what went wrong are for its log, not for this screen.
  if (!showsPicker(state, gatewayStartup)) {
    return (
      <SetupPane ref={step} step={state.step}>
        {/* This replaces the list without the step changing, so nothing
          moves focus to it; the failure has to announce itself. */}
        <div role="status" aria-live="polite" className="flex flex-col gap-3">
          <span className="flex size-11 items-center justify-center rounded-full border border-border bg-card text-foreground">
            <CircleAlert aria-hidden className="size-5" />
          </span>
          <h1 id={SETUP_HEADING_ID} className={PANE_HEADING}>
            {COULD_NOT_START}
          </h1>
        </div>
        <Button size="lg" className={PANE_BUTTON} onClick={onRetryGateway}>
          Try starting Nessa again
        </Button>
      </SetupPane>
    )
  }

  // Nothing on the list can be picked. Every reason for that can stop being
  // true while this screen is up — a sign-in done in another window, an agent
  // installed in a terminal — and until now the answer setup happened to get
  // first was the answer it kept for good. A gateway still starting is not
  // stuck: every agent's mark says so, and the list fills in once it is up.
  const stuck =
    gatewayStartup.state !== "starting" &&
    !SETUP_AGENTS.some((choice) => isChoosable(state, choice.id))
  return (
    <SetupPane ref={step} step={state.step}>
      <h1 id={SETUP_HEADING_ID} className={PANE_HEADING}>
        Choose an agent
      </h1>
      <RuntimeCopyBar progress={runtimeCopy} />
      <WithDownloads source={installations} onChanged={onRecheck} checking={checking}>
        {(downloads) => (
          <div role="group" aria-label="Agent" className="flex flex-col gap-2">
            {SETUP_AGENTS.map((choice) => {
              const readiness = agentReadiness(state, choice.id)
              const card = cardState(choice.id, readiness, downloads)
              if (card && downloads)
                return (
                  <AgentDownloadCard
                    key={choice.id}
                    id={choice.id}
                    name={choice.name}
                    card={card}
                    downloads={downloads}
                    onFocusChange={setFocusedCard}
                  />
                )
              const keyAgent = choice.id === "claude" ? choice.id : undefined
              const canSaveApiKey =
                keyAgent !== undefined &&
                readiness === "needs-authentication" &&
                apiKeys !== undefined &&
                (gatewayStartup.state === "ready" || gatewayStartup.state === "unmanaged")
              return (
                <React.Fragment key={choice.id}>
                  <AgentOption
                    id={choice.id}
                    name={choice.name}
                    readiness={readiness}
                    failure={state.readinessFailure}
                    startup={gatewayStartup}
                    selected={state.agent === choice.id}
                    onSelect={onChoose}
                    takesFocus={focusedCard === choice.id}
                    onFocusTaken={() => setFocusedCard(undefined)}
                  />
                  {canSaveApiKey ? (
                    <div className="flex flex-col gap-2">
                      <AgentApiKeyForm
                        agent={keyAgent}
                        agentName={choice.name}
                        onSave={credentials.save}
                        onSaved={onRecheck}
                      />
                    </div>
                  ) : null}
                </React.Fragment>
              )
            })}
            <LocalAiOption />
          </div>
        )}
      </WithDownloads>
      <AgentApiKeyAuditWarning failure={credentials.auditFailure} />
      {stuck ? (
        // Said once, under the list, rather than repeated on every agent:
        // each agent's mark says what it is waiting on, and this says what to
        // do about it. Polite rather than assertive, because it appears while
        // the list above it is being read.
        <p
          role="status"
          aria-live="polite"
          className="nessa-text-2 text-muted-foreground"
        >
          {state.readinessFailure
            ? "Couldn’t check your agents."
            : "Install or sign in to an agent."}
        </p>
      ) : null}
      {/* One column of identical buttons, so a second one lines up with
        Continue instead of sitting beside it at a different size. */}
      <div className="flex flex-col gap-2">
        {stuck ? (
          <Button
            type="button"
            variant="outline"
            size="lg"
            className={PANE_BUTTON}
            disabled={checking}
            onClick={onRecheck}
          >
            {checking ? "Checking…" : "Check again"}
          </Button>
        ) : null}
        <Button
          size="lg"
          className={PANE_BUTTON}
          disabled={!state.agent}
          onClick={onConfirm}
        >
          Continue
        </Button>
      </div>
    </SetupPane>
  )
}
