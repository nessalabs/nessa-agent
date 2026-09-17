import * as React from "react"
import { Check } from "lucide-react"
import { AgentMark } from "./agent-mark"
import { Keycaps } from "./keycaps"
import type { ShortcutPlatform } from "../model/shortcut-display"
import { Button } from "@nessa-ui/react/button"
import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"
import { AGENT_CHOICES, type AgentId, type OnboardingState } from "../model/onboarding"

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
      className="size-full"
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
      <div className="flex size-full flex-col items-center justify-center gap-7 px-6 pb-20 text-center">
        {children}
      </div>
      <div className="absolute inset-x-0 bottom-10 flex justify-center">{action}</div>
    </div>
  )
}

/** One selectable agent: its mark, its name, and whether it can run yet. An
 * agent no provider can run is disabled and says so, rather than being offered
 * and failing later. */
function AgentOption({
  id,
  name,
  available,
  selected,
  onSelect,
}: {
  id: AgentId
  name: string
  available: boolean
  selected: boolean
  onSelect: (id: AgentId) => void
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      // The name and its availability badge are separate elements with only a
      // margin between them, which reads as one run-together word. Name the
      // option outright so it is announced the way it is written.
      aria-label={available ? name : `${name}, coming soon`}
      disabled={!available}
      onClick={() => onSelect(id)}
      className="flex w-full items-center gap-3 rounded-xl border border-white/15 bg-background/45 p-3 text-left backdrop-blur-md transition-[background-color,border-color] outline-none hover:bg-background/65 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-50 aria-checked:border-ring aria-checked:bg-background/75"
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-border bg-card text-foreground">
        <AgentMark id={id} />
      </span>
      <span className="nessa-text-4 font-medium text-foreground">{name}</span>
      <span className="ml-auto flex items-center">
        {available ? (
          <span className="flex size-5 shrink-0 items-center justify-center rounded-full border border-border">
            {selected ? <Check aria-hidden className="size-3.5" /> : null}
          </span>
        ) : (
          <span className="nessa-text-2 text-muted-foreground">Coming soon</span>
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
  summon,
  onBegin,
  onChoose,
  onConfirm,
  onFinish,
  platform,
  onConfirmSummon,
}: {
  state: OnboardingState
  summon?: string
  /** The keyboard conventions this device writes shortcuts in. */
  platform: ShortcutPlatform
  /** Move on from the summon step, once the shortcut has been pressed. */
  onConfirmSummon: () => void
  onBegin: () => void
  onChoose: (id: AgentId) => void
  onConfirm: () => void
  onFinish: () => void
}) {
  if (state.step === "welcome") {
    return (
      <SetupStage>
        <SetupStep
          action={
            <Button size="lg" className={PILL} onClick={onBegin}>
              Get started
            </Button>
          }
        >
          <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white drop-shadow-[0_1px_16px_rgba(0,0,0,0.35)]">
            Welcome to Nessa
          </h1>
        </SetupStep>
      </SetupStage>
    )
  }

  if (state.step === "summon") {
    return (
      <SetupStage>
        {/* This step asks for a press, so there is nothing to confirm until one
          lands. Offering the way on beforehand invites a click straight past
          the only thing the step is here to teach — and the button arriving
          *because* the keys lit is what says the press worked. */}
        <SetupStep
          action={
            state.summoned ? (
              <Button size="lg" className={PILL} onClick={onConfirmSummon}>
                Continue
              </Button>
            ) : null
          }
        >
          <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white drop-shadow-[0_1px_16px_rgba(0,0,0,0.35)]">
            {summon ? "Summon it from anywhere" : "Set a summon shortcut"}
          </h1>
          {summon ? (
            <Keycaps keys={summon} platform={platform} pressed={state.summoned} />
          ) : null}
        </SetupStep>
      </SetupStage>
    )
  }

  if (state.step === "ready") {
    return (
      <SetupStage>
        <SetupStep
          action={
            <Button size="lg" className={PILL} onClick={onFinish}>
              Start using Nessa
            </Button>
          }
        >
          <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white drop-shadow-[0_1px_16px_rgba(0,0,0,0.35)]">
            You&rsquo;re all set
          </h1>
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
