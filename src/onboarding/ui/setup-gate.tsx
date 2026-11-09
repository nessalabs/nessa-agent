import type { AgentInstallations } from "../application/agent-installations"
import * as React from "react"
import { BloomField } from "@nessa-ui/react/bloom-field"
import { AgentShowcase, Onboarding, SETUP_HEADING_ID, showsPicker } from "./onboarding"
import { SetupChrome } from "./setup-chrome"
import { useIntroSound } from "./use-intro-sound"
import { useOnboarding } from "./use-onboarding"
import { useRuntimeCopy } from "./use-runtime-copy"
import { useSetupHandoff } from "./use-setup-handoff"
import { useOpening } from "./use-opening"
import { useSetupFlight } from "./use-setup-flight"
import type { AgentApiKeySink, AgentReadinessSource } from "../application/ports"
import type { SetupRecovery } from "../application/setup-recovery"
import { isOnboardingCompleted, type OnboardingState } from "../model/onboarding"
import { FIELD_GROUND, coverScale, setupField } from "../model/opening"

/** The buttons on the handoff-failure screen, which are the whole point of it:
 * a window with nothing in it but a sentence is a window someone has to kill. */
const RECOVERY_BUTTON =
  "h-10 rounded-full border border-border bg-card px-5 nessa-text-3 font-medium text-foreground transition-colors outline-none hover:bg-accent hover:text-accent-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40"

/**
 * What is on screen when setup is over and this window is still here.
 *
 * What it says and which ways out it offers come from `setupRecovery`, because
 * the two endings that reach here are different news: a panel that never came
 * up can be tried again, and a panel that is already up cannot — there the one
 * thing left is this window, and offering a retry would summon what is standing
 * behind the sentence.
 *
 * The close is always here. This window is undecorated: it has no title bar and
 * no close control of its own, so a screen without that button is a window
 * somebody has to kill.
 *
 * A close that itself fails changes nothing but the note. The screen stays,
 * with its buttons still on it, rather than throwing out of an event handler
 * and taking the last controls with it.
 */
export function HandoffFailed({
  recovery,
  onRetry,
  onSaveAgain,
  onClose,
  closeFailed,
  ref,
}: {
  /** What happened, and the one thing worth pressing about it. */
  recovery: SetupRecovery
  onRetry: () => void
  onSaveAgain: () => void
  onClose: () => void
  /** True once a close was asked for and the window system did not do it. */
  closeFailed: boolean
  /**
   * The alert itself, so whoever puts it on screen can hand it the focus the
   * setup controls just took with them. Held out here rather than focused from
   * inside, so this stays a function of its props — the way its behaviour is
   * tested at all, with no DOM in this suite.
   */
  ref?: React.Ref<HTMLDivElement>
}) {
  return (
    <div className="nessa-setup-sheet">
      <div
        ref={ref}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={SETUP_HEADING_ID}
        // Focusable only as the container of this alert, not as a stop on the
        // way through it.
        tabIndex={-1}
        className="nessa-setup-alert nessa-setup-light flex flex-col items-center justify-center gap-4 overflow-y-auto bg-background p-8 text-center text-foreground outline-none"
      >
        <h1 id={SETUP_HEADING_ID} className="nessa-text-6 font-semibold">
          {recovery.heading}
        </h1>
        <p className="nessa-text-3 text-muted-foreground">{recovery.detail}</p>
        <div className="flex flex-wrap items-center justify-center gap-3">
          {recovery.offer === "hand-over-again" ? (
            <button type="button" className={RECOVERY_BUTTON} onClick={onRetry}>
              Try again
            </button>
          ) : null}
          {recovery.offer === "save-again" ? (
            <button type="button" className={RECOVERY_BUTTON} onClick={onSaveAgain}>
              Save again
            </button>
          ) : null}
          <button type="button" className={RECOVERY_BUTTON} onClick={onClose}>
            Close this window
          </button>
        </div>
        {closeFailed ? (
          <p
            role="status"
            aria-live="polite"
            className="nessa-text-2 text-muted-foreground"
          >
            This window still would not close. If it stays in the way, quit Nessa from the
            menu bar icon and start it again.
          </p>
        ) : null}
      </div>
    </div>
  )
}

/**
 * The setup surface: first run in its own window.
 *
 * The desktop host opens this as a transparent sheet over the whole screen, so
 * setup never borrows the panel's frame, tabs or composer, and the panel is not
 * mounted behind it. The window is drawn here: a glow on the desktop that
 * becomes it (`model/opening.ts`), a light pane on the left with the step, and
 * the light itself on the right. Finishing flies it into the menu bar, then
 * shows the panel and closes this window. In a plain browser there is no
 * second window, so the same component hands over to `children` in place.
 *
 * Which is why `children` is optional, and why the setup window passes none.
 * Rendering the panel tree there mounted a second full application — its own
 * store, its own authenticated session — inside a window whose whole remaining
 * job was to close, and left it mounted for good if the handoff ever failed.
 * The setup window shows setup, then nothing.
 */
export function SetupGate({
  agents,
  apiKeys,
  installations,
  children,
  onHandOver,
}: {
  /** Where setup asks what each agent's runtime can do. Injected, so a test
   * substitutes an answer instead of a network. */
  agents: AgentReadinessSource
  /** Native secure-store boundary, present only on a surface that owns it. */
  apiKeys?: AgentApiKeySink
  installations?: AgentInstallations
  /** The panel, for a surface that has to become it in place. Omitted by the
   * desktop setup window, which closes instead. */
  children?: React.ReactNode
  /**
   * The agent setup finished on, told to whoever becomes the panel in place.
   *
   * For a surface with no host this is the only record the choice will get:
   * nothing writes it down and nothing reads it back, so a picker whose
   * selection is not carried here is a control that decides nothing. Omitted
   * by the desktop setup window, where the host keeps it and a different
   * window reads it.
   */
  onHandOver?: (agent: string) => void
}) {
  const onboarding = useOnboarding(agents)
  const runtimeCopy = useRuntimeCopy()
  useIntroSound(onboarding.active)
  // What the end of setup does to this window, and what it leaves on screen.
  // Kept out here so everything below is a function of what it reports, and
  // after the sound so the effect order is the one this surface always had.
  const completed = isOnboardingCompleted(onboarding.state)
  // A finish flies the window into the menu bar first, and setup is not over
  // for anything below until it has landed.
  const shell = React.useRef<HTMLDivElement>(null)
  const { flying } = useSetupFlight(completed, shell)
  const onScreen = onboarding.active || flying
  // The agent travels with the finish, and only with a finish: a choice made on
  // the way out of setup is not a decision, and `completeOnboarding` is what
  // keeps it. The panel reads it back from the host, because this window is
  // gone by the time it asks.
  const finishedOn = completed ? onboarding.state.agent : undefined
  const handoff = useSetupHandoff(onScreen, completed, finishedOn)

  // And told in place to a surface that has no host to write it to. Only on a
  // finish, which is the same rule the handoff applies: a choice made on the
  // way out of setup is not a decision. From an effect rather than from the
  // branch below, so rendering stays a function of what setup reports.
  //
  // Which means the panel is mounted, and its own effects have run, before
  // this one does: React runs a child's effects before its parent's. Nothing
  // rests on that ordering. What makes the handover safe is that the receiving
  // side treats a later answer as later — a host read still in flight when
  // this fires no longer clears what it wrote — so a creation that beat this
  // effect is the only thing the window costs, and that is the same window a
  // conversation created mid-setup already lives in.
  React.useEffect(() => {
    if (onScreen || finishedOn === undefined) return
    onHandOver?.(finishedOn)
  }, [onScreen, finishedOn, onHandOver])

  // The last step that was on screen, which is what the window still shows
  // while it gathers up and flies: the pane lets go of it as it leaves.
  const lastShown = React.useRef<OnboardingState>(onboarding.state)
  if (onboarding.active) lastShown.current = onboarding.state
  const shown = lastShown.current

  const beat = useOpening()
  const cover = useCoverScale(shell)
  const picking = showsPicker(shown, onboarding.gatewayStartup)
  const field = setupField(beat)

  if (!onScreen) {
    // A surface that becomes the panel in place does so now.
    if (children !== undefined) return <>{children}</>
    // The window is closing. The one thing it must not do is go quiet over a
    // handoff that left something behind — a panel that never came up, or this
    // window still standing over one that did. Either way it says which.
    if (handoff.recovery) {
      return (
        <HandoffFailed
          ref={handoff.dialog}
          recovery={handoff.recovery}
          onRetry={handoff.retry}
          onSaveAgain={handoff.saveAgain}
          onClose={handoff.close}
          closeFailed={handoff.closeFailed}
        />
      )
    }
    return null
  }
  return (
    <div className="nessa-setup-sheet">
      {/*
        Nothing dims the desktop. Setup is a window, and a room darkened to
        read it leaves a grey cast over everything the person is about to go
        back to. Leaving is something a person does on purpose: the close
        button and Escape.
      */}
      <div
        ref={shell}
        // Setup covers the whole screen, menu bar included, so it is a modal
        // dialog in fact whether or not it says so. Saying so is what puts a
        // screen reader inside it and names it with the step's own heading.
        role="dialog"
        aria-modal="true"
        aria-labelledby={SETUP_HEADING_ID}
        className="nessa-setup-window"
        data-step={shown.step}
        data-beat={beat}
        data-leaving={flying ? "" : undefined}
        // In flight the window is on its way out: nothing in it takes focus
        // or is read out, as the step it shows is already over.
        inert={flying}
        style={{ "--nessa-setup-cover": cover } as React.CSSProperties}
      >
        <BloomField
          aria-hidden="true"
          className="nessa-setup-field"
          ground={FIELD_GROUND}
          spread={field.spread}
          clear={field.clear}
          x={field.x}
        />
        {/* Above the light rather than inside the pane, so the way out does
          not move with the steps. */}
        <SetupChrome
          onBack={onboarding.canGoBack && !flying ? onboarding.back : undefined}
          onClose={flying ? undefined : onboarding.dismiss}
        />
        <Onboarding
          state={shown}
          open={beat === "open" && !flying}
          apiKeys={apiKeys}
          installations={installations}
          accelerator={onboarding.accelerator}
          onBegin={onboarding.begin}
          onChoose={onboarding.choose}
          onConfirm={onboarding.confirm}
          onFinish={onboarding.finish}
          onAcceptLinger={onboarding.acceptLinger}
          lingerPending={onboarding.lingerPending}
          onRecheck={onboarding.recheck}
          onRetryGateway={onboarding.retryGatewayStartup}
          gatewayStartup={onboarding.gatewayStartup}
          checking={onboarding.checking}
          platform={onboarding.platform}
          runtimeCopy={runtimeCopy}
        />
        {/* The picture beside the pane. The agent being chosen stands on it on
          its card; the card is built once the window is open, so Three.js
          loads after the opening rather than during it, and stays built
          across steps so the picker never waits on it. */}
        <div
          aria-hidden="true"
          className="nessa-setup-art"
          data-showing={picking ? "" : undefined}
        >
          {beat === "open" ? <AgentShowcase chosen={shown.agent} /> : null}
        </div>
      </div>
    </div>
  )
}

/**
 * How much larger than itself the window is drawn while it is still a glow,
 * kept to the screen it is on (`coverScale`). Measured from the window's own
 * box, which a transform does not change.
 */
function useCoverScale(shell: React.RefObject<HTMLElement | null>): number {
  const [cover, setCover] = React.useState(3)
  React.useLayoutEffect(() => {
    const element = shell.current
    if (!element) return
    const measure = () =>
      setCover(
        coverScale(
          { width: innerWidth, height: innerHeight },
          { width: element.offsetWidth, height: element.offsetHeight },
        ),
      )
    measure()
    addEventListener("resize", measure)
    return () => removeEventListener("resize", measure)
  }, [shell])
  return cover
}
