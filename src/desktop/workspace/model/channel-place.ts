/**
 * Where a channel runs its agents, in the gateway's words (`channel.setPlacement`):
 * `local`, `boat`, `pool`, or one computer by its place name, an SSH
 * destination the gateway serves or a paired computer's `peer:<key>`. Pure;
 * the gateway decides which computers a channel may name. How a person reads
 * one is the UI's `channelPlaceWords`.
 */
export type ChannelPlace = string

/** Which kind of place a channel place is: one computer for any name. */
export type ChannelPlaceKind = "local" | "boat" | "pool" | "computer"

export function channelPlaceKind(place: ChannelPlace): ChannelPlaceKind {
  return place === "local" || place === "boat" || place === "pool" ? place : "computer"
}
