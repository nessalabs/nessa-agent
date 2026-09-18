import * as React from "react"
import { Check } from "lucide-react"
import { AgentMark } from "./agent-mark"
import { Keycaps } from "./keycaps"
import { useHeldKeys } from "./use-held-keys"
import type { ShortcutPlatform } from "../model/shortcut-display"
import { Button } from "@nessa-ui/react/button"
import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"
import {
  AGENT_CHOICES,
  agentReadiness,
  isChoosable,
  type AgentId,
  type AgentReadiness,
  type AgentReadinessFailure,
  type OnboardingState,
} from "../model/onboarding"
import { revealChunks } from "../model/reveal-text"

/** Setup's primary action, sized the same on every step. */
const PILL =
  "nessa-setup-arrive h-12 min-w-56 rounded-full bg-white px-10 nessa-text-5 font-medium text-neutral-950 shadow-lg hover:bg-white/90"

/** The id the setup window's `aria-labelledby` points at. Every step titles the
 * dialog with its own heading, so there is exactly one of these on screen. */
export const SETUP_HEADING_ID = "nessa-setup-heading"

/**
 * The wash every setup step is painted on.
 *
 * It is decorative: inert to the pointer, hidden from assistive technology by
 * the component, and still under a reduced-motion preference. Its pigments
 * travel, so anything laid straight on it has to stay legible wherever they
 * go — plain white type over the palette's own lighter stops measures about
 * 1.7:1, which is why the type carries a stacked shadow rather than one soft
 * one: a tight, near-opaque layer close to the glyph for an edge that holds
 * regardless of what is behind it, and a wider, softer one for depth.
 */
function SetupStage({ children }: { children: React.ReactNode }) {
  return (
    <MorphingMeshGradient
      colors={morphingMeshGradientPresets.glass}
      type="mesh"
      speed={1.1}
      blur={88}
      className="nessa-setup-stage size-full"
    >
      {/* Setup's controls are always the light treatment: a dark pill over
        these pigments reads as a hole punched in the wash, and the light
        palette is what the design system's own components are built against
        here. Headings set their colour explicitly for the same reason. */}
      <div className="nessa-setup-light relative flex size-full min-h-0 items-center justify-center p-5">
        {children}
      </div>
    </MorphingMeshGradient>
  )
}

/**
 * The glass pane for a step that carries more than a line and a button.
 *
 * A list of options cannot be read off the moving wash, so it gets a surface.
 * The pane is scoped to the light palette because a dark slab over these
 * pigments reads as a hole rather than glass, and because its own components
 * then keep the contrast they were built for.
 *
 * It scrolls rather than clips. At a large text size or a high UI scale the
 * list and its Continue button are taller than the window, and a pane that
 * hid the button made setup impossible to finish.
 */
const SetupPanel = React.forwardRef<HTMLDivElement, { children: React.ReactNode }>(
  function SetupPanel({ children }, ref) {
    return (
      <div
        ref={ref}
        tabIndex={-1}
        className="nessa-setup-light relative flex max-h-full w-full max-w-sm flex-col gap-5 overflow-y-auto rounded-2xl border border-white/40 bg-background/60 p-6 text-foreground shadow-2xl ring-1 ring-black/5 outline-none backdrop-blur-2xl backdrop-saturate-150"
      >
        {/* The lit top edge that reads as a pane of glass catching light. */}
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-white/45 to-transparent"
        />
        {children}
      </div>
    )
  },
)

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
 * A heading that resolves a few letters at a time.
 *
 * Whole words arrive in too few, too large steps to read as a line being said.
 * A couple of characters at a time is continuous — and because each piece
 * takes far longer to resolve than the gap between its neighbours, what is
 * seen is one movement crossing the line rather than pieces arriving in turn.
 *
 * Each piece carries its place in the order and the stylesheet turns that into
 * a delay, so the pace of the line is one number in one file.
 *
 * The pieces exist only to be animated, so the heading carries the line as its
 * own name and every piece is hidden — the same arrangement `Keycaps` uses.
 * Without it a screen reader reads two-character fragments and the product's
 * own name arrives in halves.
 *
 * The shadow that keeps white type legible moves to the pieces for the same
 * reason it exists at all — it has to be on whatever is actually being
 * filtered, or the animation replaces it.
 */
function SetupHeading({ children }: { children: string }) {
  return (
    <h1
      id={SETUP_HEADING_ID}
      data-words
      aria-label={children}
      className="nessa-setup-title font-semibold text-white"
    >
      {revealChunks(children).map((chunk, index) => (
        <span
          // Chunks repeat within a line, so position is the only identity.
          key={`${chunk}-${index}`}
          aria-hidden="true"
          className="nessa-setup-word"
          style={{ "--nessa-word": index } as React.CSSProperties}
        >
          {chunk}
        </span>
      ))}
    </h1>
  )
}

/**
 * A step that is a line and a way on.
 *
 * Everything is in normal flow and the column scrolls. The action used to be
 * positioned against the box so the heading could sit at its centre whether or
 * not there was an action yet; that bought a still heading at the price of a
 * button that could be clipped clean off the window at a large text size, with
 * nothing scrolling to reach it. Since every step now always offers a way on,
 * the jump it was avoiding no longer happens.
 */
const SetupStep = React.forwardRef<
  HTMLDivElement,
  { children: React.ReactNode; action?: React.ReactNode }
>(function SetupStep({ children, action }, ref) {
  return (
    <div
      ref={ref}
      tabIndex={-1}
      className="flex size-full min-h-0 flex-col items-center justify-center gap-7 overflow-y-auto px-6 py-10 text-center outline-none"
    >
      {children}
      <div className="flex shrink-0 justify-center pt-3">{action}</div>
    </div>
  )
})

/**
 * What an agent that cannot be picked is waiting on.
 *
 * Every one of these is shown on the agent itself rather than as a message
 * elsewhere, because the reason belongs to the thing it is about — and most of
 * them are fixable, which is only useful if the person can tell which.
 *
 * "Nothing answered" is not a fact about the agent and does not read as one.
 * Collapsing a gateway that is not running into the same words as an agent
 * Nessa does not support yet told someone their setup was fine when it was not.
 */
function readinessNote(
  readiness: AgentReadiness,
  failure: AgentReadinessFailure | undefined,
): string | undefined {
  if (readiness === "ready") return undefined
  if (readiness === "not-supported") return "Coming soon"
  if (readiness === "needs-authentication") return "Needs sign-in"
  if (readiness === "not-installed") return "Not installed"
  // Not "not installed": this Nessa was not set up to run it, and the agent may
  // be sitting on the machine already. Telling someone to install what they
  // have is advice that cannot work however many times they take it.
  if (readiness === "not-configured") return "Not set up here"
  if (failure === "unreachable") return "Can’t reach Nessa"
  if (failure === "unreadable") return "Unexpected answer"
  return "Checking…"
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
  selected,
  onSelect,
}: {
  id: AgentId
  name: string
  readiness: AgentReadiness
  failure: AgentReadinessFailure | undefined
  selected: boolean
  onSelect: (id: AgentId) => void
}) {
  const note = readinessNote(readiness, failure)
  return (
    <button
      type="button"
      // Plain toggles rather than a radio group. The group announced the
      // arrow-key pattern a radio group promises and implemented none of it:
      // arrows did nothing and every option was its own tab stop. Saying what
      // the markup actually does is the smaller of the two honest changes.
      aria-pressed={selected}
      aria-disabled={note ? true : undefined}
      // The name and its note are separate elements with only a margin between
      // them, which reads as one run-together word. Name the option outright so
      // it is announced the way it is written.
      aria-label={note ? `${name}, ${note.toLowerCase()}` : name}
      onClick={() => {
        if (note) return
        onSelect(id)
      }}
      className="flex w-full items-center gap-3 rounded-xl border border-white/15 bg-background/45 p-3 text-left backdrop-blur-md transition-[background-color,border-color] outline-none hover:bg-background/65 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 aria-disabled:cursor-default aria-disabled:opacity-60 aria-disabled:hover:bg-background/45 aria-pressed:border-ring aria-pressed:bg-background/75"
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-border bg-card text-foreground">
        <AgentMark id={id} />
      </span>
      <span className="nessa-text-4 font-medium text-foreground">{name}</span>
      <span className="ml-auto flex items-center">
        {note ? (
          <span className="nessa-text-2 text-muted-foreground">{note}</span>
        ) : (
          <span className="flex size-5 shrink-0 items-center justify-center rounded-full border border-border">
            {selected ? <Check aria-hidden className="size-3.5" /> : null}
          </span>
        )}
      </span>
    </button>
  )
}

/**
 * First-run setup for the panel.
 *
 * This renders and dispatches; it holds no state and decides nothing. Choosing
 * an agent records the choice and nothing more — no credential is collected and
 * no provider is started — so the copy promises setup, not a ready agent.
 */
export function Onboarding({
  state,
  accelerator,
  onBegin,
  onChoose,
  onConfirm,
  onFinish,
  onRecheck,
  checking = false,
  platform,
}: {
  state: OnboardingState
  accelerator?: string
  /** The keyboard conventions this device writes shortcuts in. */
  platform: ShortcutPlatform
  onBegin: () => void
  onChoose: (id: AgentId) => void
  onConfirm: () => void
  onFinish: () => void
  /** Ask the runtimes again, for whoever has just fixed what was wrong. */
  onRecheck: () => void
  /** True while an ask is in flight. The button that starts one says so and
   * stops taking presses, because each ask runs a real probe per agent and a
   * button that looks inert invites being pressed again. */
  checking?: boolean
}) {
  // Called unconditionally, as a hook must be; it only listens on the step
  // that has keys to light.
  const held = useHeldKeys(state.step === "summon")

  // Each step is a different subtree, so a button that moves setup on takes its
  // own focus with it and leaves the caret on `<body>`. A keyboard user then
  // tabs from the top of the document every time and a screen reader is told
  // nothing at all. The new step takes the focus instead.
  const step = React.useRef<HTMLDivElement>(null)
  React.useEffect(() => {
    step.current?.focus()
  }, [state.step])

  if (state.step === "welcome") {
    return (
      <SetupStage>
        <SetupStep
          ref={step}
          action={
            <Button
              size="lg"
              className={PILL}
              onClick={onBegin}
              // After the last of the line, so the sentence finishes being
              // said before anything is asked.
              style={
                {
                  "--nessa-word": revealChunks("Welcome to Nessa").length,
                } as React.CSSProperties
              }
            >
              Get started
            </Button>
          }
        >
          <SetupHeading>Welcome to Nessa</SetupHeading>
        </SetupStep>
      </SetupStage>
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
      <SetupStage>
        <SetupStep
          ref={step}
          action={
            taught ? (
              <Button size="lg" className={PILL} onClick={onFinish}>
                Start using Nessa
              </Button>
            ) : (
              <Button
                variant="ghost"
                size="lg"
                onClick={onFinish}
                className="nessa-setup-arrive rounded-full px-6 text-white/85 hover:bg-white/15 hover:text-white"
              >
                Skip this step
              </Button>
            )
          }
        >
          <h1
            id={SETUP_HEADING_ID}
            className="nessa-setup-title nessa-setup-arrive font-semibold text-white"
          >
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
        </SetupStep>
      </SetupStage>
    )
  }

  // Nothing on the list can be picked. Every reason for that can stop being
  // true while this screen is up — a gateway still starting, a sign-in done in
  // another window, an agent installed in a terminal — and until now the answer
  // setup happened to get first was the answer it kept for good.
  const stuck = !AGENT_CHOICES.some((choice) => isChoosable(state, choice.id))
  return (
    <SetupStage>
      <SetupPanel ref={step}>
        <h1 id={SETUP_HEADING_ID} className="nessa-text-6 font-semibold text-foreground">
          Choose an agent
        </h1>
        <div role="group" aria-label="Agent" className="flex flex-col gap-2">
          {AGENT_CHOICES.map((choice) => (
            <AgentOption
              key={choice.id}
              id={choice.id}
              name={choice.name}
              readiness={agentReadiness(state, choice.id)}
              failure={state.readinessFailure}
              selected={state.agent === choice.id}
              onSelect={onChoose}
            />
          ))}
        </div>
        {stuck ? (
          // Said once, under the list, rather than repeated on every agent: the
          // per-agent note says what each one is waiting on, and this says what
          // to do about it. Polite rather than assertive, because it appears
          // while the list beneath it is being read.
          <div role="status" aria-live="polite" className="flex flex-col gap-2">
            <p className="nessa-text-2 text-muted-foreground">
              {state.readinessFailure
                ? "Nessa could not ask what is installed here."
                : "No agent here can start yet."}{" "}
              Sign in or start one, then check again.
            </p>
            <Button
              type="button"
              variant="outline"
              className="rounded-full"
              disabled={checking}
              onClick={onRecheck}
            >
              {checking ? "Checking…" : "Check again"}
            </Button>
          </div>
        ) : null}
        <Button
          size="lg"
          className="rounded-full"
          disabled={!state.agent}
          onClick={onConfirm}
        >
          Continue
        </Button>
      </SetupPanel>
    </SetupStage>
  )
}
