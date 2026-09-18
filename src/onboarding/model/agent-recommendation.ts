/** What first-run setup should suggest when somebody has no agent to pick.
 *
 * Setup lists every agent and lets the person choose. That works when they
 * already have one. Someone who has never installed a coding agent reaches the
 * picker, finds nothing selectable, and is told to go away and install
 * something before Nessa can do anything at all — which is the worst moment in
 * the whole product to hand somebody a homework assignment.
 *
 * Nessa can install an agent itself, so on that screen there is something
 * better to say. This module decides what.
 *
 * Deliberately not written around one agent's name. Which agents Nessa can
 * fetch is a fact the server owns — it pins the releases it has tested — and
 * naming one here would put that truth in two places that could disagree.
 */

import { AGENT_CHOICES, agentChoice, isChoosable } from "./onboarding"
import type { AgentId, OnboardingState } from "./onboarding"

/**
 * What setup should offer.
 *
 * Three cases, because they are three different screens. `choose` is the
 * ordinary picker. `install` is the picker with an offer on it. `nothing` is
 * the picker with no offer, which is what an unanswered question has to look
 * like — see below.
 */
export type AgentRecommendation =
  /** At least one agent can start. The person picks; Nessa suggests nothing. */
  | { readonly kind: "choose" }
  /** No agent can start, and this is one Nessa can install. */
  | { readonly kind: "install"; readonly agent: AgentId }
  /** No agent can start and there is nothing to offer, or nobody answered. */
  | { readonly kind: "nothing" }

/**
 * Whether any listed agent is ready to start.
 *
 * `isChoosable` is the same question the picker asks before it lets somebody
 * select a row, and it is reused rather than restated so that the offer and the
 * picker cannot come to disagree about what "ready" means.
 */
function anythingReady(state: OnboardingState): boolean {
  return AGENT_CHOICES.some((choice) => isChoosable(state, choice.id))
}

/**
 * Whether anybody actually answered about the agents on this machine.
 *
 * Not the same question as whether a report arrived. A report is an object, and
 * an empty one is truthy: a gateway that answers `{"agents":[]}`, or one that
 * has gained a readiness state this build does not recognise — which the
 * adapter drops rather than passes on — produces a report that says nothing
 * about any agent. Treating that as "this machine has no agents" would offer
 * somebody a large download on the strength of a question nobody answered.
 */
function anybodyAnswered(state: OnboardingState): boolean {
  return AGENT_CHOICES.some((choice) => state.readiness?.[choice.id] !== undefined)
}

/**
 * Whether Nessa could actually put this agent in front of somebody.
 *
 * Being installable is not enough. The server can pin a release for an agent
 * this build has no adapter for — that is exactly the state an agent passes
 * through while it is being added — and offering to download one would end with
 * a hundred megabytes on disk and a row in the picker that still cannot be
 * selected.
 */
function canBeOffered(agent: AgentId): boolean {
  return agentChoice(agent)?.supported === true
}

/**
 * What to suggest, given what the runtimes said and what Nessa can install.
 *
 * `installable` is the list of agents Nessa can fetch, which is a fact about
 * this build rather than about the machine. Passing it in rather than naming an
 * agent here keeps that single source of truth on the side that owns it.
 *
 * Deliberately not offered on the strength of a missing answer: a report that
 * never arrived, or one that arrived saying nothing about any listed agent, is
 * not evidence that somebody has no agent. Offering a large download to a
 * person whose gateway was briefly unreachable would be acting on a question
 * nobody answered. That is the rule the rest of setup follows, and it is why an
 * unanswered ask reads as no recommendation rather than as an empty machine.
 */
export function recommendAgent(
  state: OnboardingState,
  installable: readonly AgentId[],
): AgentRecommendation {
  // Nobody said anything about any agent. Not an empty machine: an unanswered
  // question, which reads the same whether the ask failed or came back with
  // nothing in it.
  if (!anybodyAnswered(state)) return { kind: "nothing" }
  if (anythingReady(state)) return { kind: "choose" }
  const offer = installable.find(canBeOffered)
  return offer ? { kind: "install", agent: offer } : { kind: "nothing" }
}
