/** Where the refused run's agent ran, as its lease says. */
export type LeaseEnvironment = "here" | "ssh" | "peer" | "boat"

/**
 * Which sign-in the card offers: this computer's, or a ChatGPT account the
 * gateway hands Codex on another computer (`chatgpt-sign-in.ts`), which this
 * computer's sign-in does not reach.
 */
export type SignInAction = "computer" | "chatgpt"

export function signInAction(
  provider: "claude" | "codex",
  environment: LeaseEnvironment | undefined,
): SignInAction {
  if (provider === "claude") return "computer"
  return environment === "ssh" || environment === "peer" || environment === "boat"
    ? "chatgpt"
    : "computer"
}
