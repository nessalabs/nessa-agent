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

import { AGENT_CHOICES, agentChoice, agentReadiness, isChoosable } from "./onboarding"
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
 * The one answer that means installing this agent would help.
 *
 * Exactly `not-installed`, and the narrowness is the point: this decides
 * whether to propose a download, so it has to mean the agent is not here.
 *
 * `needs-authentication` is the near miss. It reads like a problem an offer
 * could solve, and it is not: the readiness type spells it out as *installed*,
 * but with nothing signed in. Downloading the version that is already on the
 * machine would find it, fetch nothing, and leave the row exactly as
 * unselectable as before, having told somebody their sign-in problem was a
 * missing install. `ready` is the opposite answer, `unknown` is the absence of
 * one, and `not-supported` is a fact about Nessa's listing rather than
 * something a runtime reported.
 *
 * Read as a single answer rather than as "anything that is not `ready`",
 * because the two decisions on this screen take opposite defaults and each
 * takes the careful one. Letting somebody *pick* an agent needs a definite yes,
 * so a state nobody has heard of does not count as one. Proposing a
 * hundred-megabyte *download* needs a definite "it is not here", so a state
 * nobody has heard of does not count as that either.
 */
function isMissing(state: OnboardingState, agent: AgentId): boolean {
  return agentReadiness(state, agent) === "not-installed"
}

/**
 * Whether Nessa could actually put this agent in front of somebody, and has
 * been told it needs to.
 *
 * Two conditions, and both are about this agent rather than about the machine
 * in general. Being installable is not enough: the server can pin a release for
 * an agent this build has no adapter for — exactly the state an agent passes
 * through while it is being added — and offering to download one would end with
 * a hundred megabytes on disk and a row in the picker that still cannot be
 * selected.
 *
 * Neither is somebody else's answer enough. Read through `agentReadiness`, the
 * same way the picker reads it, so that an answer the listing overrides is not
 * quietly treated as evidence. A gateway that answered about codex and said
 * nothing about claude has not told Nessa anything about claude, and offering
 * to install claude on the strength of it would be a download proposed to
 * somebody whose claude is sitting there working.
 */
function canBeOffered(state: OnboardingState, agent: AgentId): boolean {
  return agentChoice(agent)?.supported === true && isMissing(state, agent)
}

/**
 * What to suggest, given what the runtimes said and what Nessa can install.
 *
 * `installable` is the list of agents Nessa can fetch, which is a fact about
 * this build rather than about the machine. Passing it in rather than naming an
 * agent here keeps that single source of truth on the side that owns it.
 *
 * Deliberately not offered on the strength of a missing answer. A report that
 * never arrived, one that arrived empty, and one that answered about a
 * different agent are all the same thing here: nobody said this agent cannot
 * start. Offering a large download to a person whose gateway was briefly
 * unreachable, or whose own agent is sitting there working, would be acting on
 * a question nobody answered. That is the rule the rest of setup follows, and
 * it is why an unanswered ask reads as no recommendation rather than as an
 * empty machine.
 */
export function recommendAgent(
  state: OnboardingState,
  installable: readonly AgentId[],
): AgentRecommendation {
  if (anythingReady(state)) return { kind: "choose" }
  // No separate guard for "nobody answered". An agent is offered only when it
  // was itself answered about, so a report that arrived empty, an ask that
  // failed, and a machine nobody has asked about yet all fall through here on
  // their own — rather than through a check that could pass on one agent's
  // answer and then offer a different one.
  const offer = installable.find((agent) => canBeOffered(state, agent))
  return offer ? { kind: "install", agent: offer } : { kind: "nothing" }
}
