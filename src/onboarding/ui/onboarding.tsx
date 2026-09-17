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
  type AgentId,
  type AgentReadiness,
  type OnboardingState,
} from "../model/onboarding"
import { revealChunks } from "../model/reveal-text"

/** Setup's primary action, sized the same on every step. */
const PILL =
  "nessa-setup-arrive h-12 min-w-56 rounded-full bg-white px-10 nessa-text-5 font-medium text-neutral-950 shadow-lg hover:bg-white/90"

/**
 * The wash every setup step is painted on.
 *
 * It is decorative: inert to the pointer, hidden from assistive technology by
 * the component, and still under a reduced-motion preference. Its pigments
 * travel, so anything laid straight on it has to stay legible wherever they go
 * — which large, heavy, shadowed type does and small body copy does not.
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
 */
function SetupPanel({ children }: { children: React.ReactNode }) {
  return (
    <div className="nessa-setup-light relative flex w-full max-w-sm flex-col gap-5 overflow-hidden rounded-2xl border border-white/40 bg-background/60 p-6 text-foreground shadow-2xl ring-1 ring-black/5 backdrop-blur-2xl backdrop-saturate-150">
      {/* The lit top edge that reads as a pane of glass catching light. */}
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-white/45 to-transparent"
      />
      {children}
    </div>
  )
}

/**
 * What the summon step says, which is a different thing at each press.
 *
 * The same keys do both jobs, so the copy has to name the one that has not
 * been shown yet: first that it summons, then that the very same press puts it
 * away, then that the pair of them is the whole trick.
 */
function summonHeading(summon: OnboardingState["summon"], configured: boolean) {
  if (!configured) return "Set a summon shortcut"
  if (summon === undefined) return "Summon it from anywhere"
  if (summon === "shown") return "Press it again to hide"
  return "That\u2019s all it takes"
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
 * The shadow that keeps white type legible moves to the pieces for the same
 * reason it exists at all — it has to be on whatever is actually being
 * filtered, or the animation replaces it.
 */
function SetupHeading({ children }: { children: string }) {
  return (
    <h1 data-words className="nessa-setup-title font-semibold text-white">
      {revealChunks(children).map((chunk, index) => (
        <span
          // Chunks repeat within a line, so position is the only identity.
          key={`${chunk}-${index}`}
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
 * The heading is centred in the box and the action is anchored to the bottom
 * of it, rather than the heading being centred in whatever is left above the
 * action. Otherwise a step whose action has not appeared yet — the summon step
 * before the shortcut is pressed — hangs its heading too high, and the heading
 * jumps when the action arrives.
 */
function SetupStep({
  children,
  action,
}: {
  children: React.ReactNode
  action?: React.ReactNode
}) {
  return (
    <div className="relative size-full min-h-0">
      {/* No padding for the action below: it is positioned against the box, not
        laid out in this column, so reserving room for it here only pushed the
        content off the box's centre — by exactly half of whatever was
        reserved. */}
      <div className="flex size-full flex-col items-center justify-center gap-7 px-6 text-center">
        {children}
      </div>
      <div className="absolute inset-x-0 bottom-10 flex justify-center">{action}</div>
    </div>
  )
}

/**
 * What an agent that cannot be picked is waiting on.
 *
 * Every one of these is shown on the agent itself rather than as a message
 * elsewhere, because the reason belongs to the thing it is about — and two of
 * the three are fixable, which is only useful if the person can tell which.
 */
function readinessNote(
  readiness: AgentReadiness,
  supported: boolean,
): string | undefined {
  if (readiness === "ready") return undefined
  // No adapter is the one reason that will not change by doing anything here.
  if (!supported) return "Coming soon"
  if (readiness === "needs-authentication") return "Needs sign-in"
  if (readiness === "not-installed") return "Not installed"
  // Nothing heard back — the gateway is not running, or has not answered yet.
  // Saying so is better than naming a cause that would be a guess.
  return "Not available"
}

/** One listed agent: its mark, its name, and what it is waiting on. An agent
 * that cannot run is shown with the reason rather than hidden — a missing
 * Claude is a question, and a Claude that says it needs signing in is an
 * answer. */
function AgentOption({
  id,
  name,
  supported,
  readiness,
  selected,
  onSelect,
}: {
  id: AgentId
  name: string
  supported: boolean
  readiness: AgentReadiness
  selected: boolean
  onSelect: (id: AgentId) => void
}) {
  const note = readinessNote(readiness, supported)
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      // The name and its note are separate elements with only a margin between
      // them, which reads as one run-together word. Name the option outright so
      // it is announced the way it is written.
      aria-label={note ? `${name}, ${note.toLowerCase()}` : name}
      disabled={Boolean(note)}
      onClick={() => onSelect(id)}
      className="flex w-full items-center gap-3 rounded-xl border border-white/15 bg-background/45 p-3 text-left backdrop-blur-md transition-[background-color,border-color] outline-none hover:bg-background/65 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-50 aria-checked:border-ring aria-checked:bg-background/75"
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
}) {
  // Called unconditionally, as a hook must be; it only listens on the step
  // that has keys to light.
  const held = useHeldKeys(state.step === "summon")

  if (state.step === "welcome") {
    return (
      <SetupStage>
        <SetupStep
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
    return (
      <SetupStage>
        {/* This step asks for presses, so there is nothing to confirm until
          both have landed. Offering the way on beforehand invites a click
          straight past the only thing the step is here to teach — and the
          button arriving *because* the keys lit is what says they worked. */}
        <SetupStep
          action={
            state.summon === "hidden" ? (
              <Button size="lg" className={PILL} onClick={onFinish}>
                Start using Nessa
              </Button>
            ) : null
          }
        >
          <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white">
            {summonHeading(state.summon, Boolean(accelerator))}
          </h1>
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

  return (
    <SetupStage>
      <SetupPanel>
        <h1 className="nessa-text-6 font-semibold text-foreground">Choose an agent</h1>
        <div role="radiogroup" aria-label="Agent" className="flex flex-col gap-2">
          {AGENT_CHOICES.map((choice) => (
            <AgentOption
              key={choice.id}
              {...choice}
              readiness={agentReadiness(state, choice.id)}
              selected={state.agent === choice.id}
              onSelect={onChoose}
            />
          ))}
        </div>
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
