/**
 * Claude in a Boat VM cannot use this computer's sign-in: Boat gives a VM
 * made from Nessa's image none of the owner's logins (ADR 501). Its card
 * mints a sign-in for Boat instead, which the gateway keeps for every later
 * serve of a Boat VM.
 */

/** Where the refused run's agent ran, as its lease says. */
export type LeaseEnvironment = "here" | "ssh" | "peer" | "boat"

/** Which sign-in the card offers: this computer's, or one minted for Boat. */
export type SignInAction = "computer" | "boat"

export function signInAction(
  provider: "claude" | "codex",
  environment: LeaseEnvironment | undefined,
): SignInAction {
  return provider === "claude" && environment === "boat" ? "boat" : "computer"
}

/** Why a Boat sign-in did not complete: minting ended without a token, or
 * the gateway did not keep the one minted. */
export class BoatSignInFailure extends Error {
  constructor(
    readonly stage: "minted" | "saved",
    options?: { cause?: unknown },
  ) {
    super(
      stage === "minted" ? "Claude sign-in did not complete" : "Sign-in not saved",
      options,
    )
    this.name = "BoatSignInFailure"
  }
}

/** A Claude sign-in for Boat, minted by the host and kept by the gateway. */
export interface BoatClaudeSignIn {
  available(): Promise<boolean>
  signIn(): Promise<void>
}

/** The token goes from `mint` straight to `save` and is held nowhere else. */
export function boatClaudeSignIn(ports: {
  available: () => Promise<boolean>
  mint: () => Promise<string>
  save: (token: string) => Promise<unknown>
}): BoatClaudeSignIn {
  return {
    available: async () => {
      try {
        return await ports.available()
      } catch {
        return false
      }
    },
    signIn: async () => {
      let token: string
      try {
        token = await ports.mint()
      } catch (cause) {
        throw new BoatSignInFailure("minted", { cause })
      }
      try {
        await ports.save(token)
      } catch (cause) {
        throw new BoatSignInFailure("saved", { cause })
      }
    },
  }
}
