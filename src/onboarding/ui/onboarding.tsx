import * as React from "react"
import { Check, X } from "lucide-react"
import { Button } from "@nessa-ui/react/button"
import {
  MorphingMeshGradient,
  morphingMeshGradientPresets,
} from "@nessa-ui/react/morphing-mesh-gradient"
import { AGENT_CHOICES, type AgentId, type OnboardingState } from "../model/onboarding"

/**
 * The wash every setup step is painted on.
 *
 * It is decorative: inert to the pointer, hidden from assistive technology by
 * the component, and still under a reduced-motion preference. Its pigments
 * travel, so anything laid straight on it has to stay legible wherever they go
 * — which large, heavy, shadowed type does and small body copy does not.
 */
function SetupStage({
  children,
  onDismiss,
}: {
  children: React.ReactNode
  onDismiss?: () => void
}) {
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
        {onDismiss ? (
          <button
            type="button"
            aria-label="Skip setup"
            onClick={onDismiss}
            className="absolute top-3 right-3 z-10 flex size-8 items-center justify-center rounded-full text-white/80 transition-colors outline-none hover:bg-white/20 hover:text-white focus-visible:ring-[3px] focus-visible:ring-white/50"
          >
            <X aria-hidden className="size-4" />
          </button>
        ) : null}
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

/** One selectable agent. An agent no provider can run is disabled and says so,
 * rather than being offered and failing later. */
function AgentOption({
  id,
  name,
  summary,
  available,
  selected,
  onSelect,
}: {
  id: AgentId
  name: string
  summary: string
  available: boolean
  selected: boolean
  onSelect: (id: AgentId) => void
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      // The visible name and its availability badge are separate elements with
      // only a margin between them, which reads as one run-together word. Name
      // the option outright so it is announced the way it is written.
      aria-label={available ? name : `${name}, coming soon`}
      aria-describedby={`onboarding-agent-${id}-summary`}
      disabled={!available}
      onClick={() => onSelect(id)}
      className="flex w-full items-start gap-3 rounded-xl border border-white/15 bg-background/45 p-4 text-left backdrop-blur-md transition-[background-color,border-color] outline-none hover:bg-background/65 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-50 aria-checked:border-ring aria-checked:bg-background/75"
    >
      <span className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border border-border">
        {selected ? <Check aria-hidden className="size-3.5" /> : null}
      </span>
      <span className="flex min-w-0 flex-col gap-1">
        <span className="nessa-text-4 font-medium text-foreground">
          {name}
          {available ? null : (
            <span className="ml-2 nessa-text-2 font-normal text-muted-foreground">
              Coming soon
            </span>
          )}
        </span>
        <span
          id={`onboarding-agent-${id}-summary`}
          className="nessa-text-2 text-muted-foreground"
        >
          {summary}
        </span>
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
  onDismiss,
}: {
  state: OnboardingState
  summon?: string
  onBegin: () => void
  onChoose: (id: AgentId) => void
  onConfirm: () => void
  onFinish: () => void
  /** Leave setup without finishing it. Setup has no window chrome of its own,
   * so this is the corner control that stands in for it. */
  onDismiss?: () => void
}) {
  if (state.step === "welcome") {
    return (
      <SetupStage onDismiss={onDismiss}>
        <div className="flex size-full min-h-0 flex-col items-center justify-end gap-10 pb-10 text-center">
          <div className="flex flex-1 items-center">
            <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white drop-shadow-[0_1px_16px_rgba(0,0,0,0.35)]">
              Welcome to Nessa
            </h1>
          </div>
          <Button
            size="lg"
            className="nessa-setup-arrive rounded-full bg-white px-8 text-neutral-950 shadow-lg hover:bg-white/90"
            onClick={onBegin}
          >
            Get started
          </Button>
        </div>
      </SetupStage>
    )
  }

  if (state.step === "summon") {
    return (
      <SetupStage onDismiss={onDismiss}>
        <div className="flex size-full min-h-0 flex-col items-center justify-end gap-10 pb-10 text-center">
          <div className="flex flex-1 flex-col items-center justify-center gap-5 px-4">
            <h1 className="nessa-setup-title nessa-setup-arrive font-semibold text-white drop-shadow-[0_1px_16px_rgba(0,0,0,0.35)]">
              {summon ? "Summon it from anywhere" : "Set a summon shortcut"}
            </h1>
            {summon ? (
              <kbd className="nessa-setup-arrive rounded-full border border-white/40 bg-white/25 px-5 py-2 font-sans nessa-text-5 font-medium tracking-wide text-white shadow-lg backdrop-blur-md">
                {summon}
              </kbd>
            ) : null}
          </div>
          <Button
            size="lg"
            className="nessa-setup-arrive rounded-full bg-white px-8 text-neutral-950 shadow-lg hover:bg-white/90"
            onClick={onFinish}
          >
            Start using Nessa
          </Button>
        </div>
      </SetupStage>
    )
  }

  return (
    <SetupStage onDismiss={onDismiss}>
      <SetupPanel>
        <div className="flex flex-col gap-2">
          <h1 className="nessa-text-6 font-semibold text-foreground">Choose an agent</h1>
          <p className="nessa-text-2 text-muted-foreground">
            Nessa runs your conversations through this agent. You can change it later in
            settings.
          </p>
        </div>
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
