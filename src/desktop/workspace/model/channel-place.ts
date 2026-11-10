/**
 * Where a channel runs its agents, in the gateway's words (`channel.setPlacement`):
 * `local`, `boat`, `pool`, or one computer by its place name, an SSH
 * destination the gateway serves or a paired computer's `peer:<key>`. Pure;
 * the gateway decides which computers a channel may name.
 */
import { environmentPlace } from "@nessa/client"

export type ChannelPlace = string

/** Which kind of place a channel place is: one computer for any name. */
export type ChannelPlaceKind = "local" | "boat" | "pool" | "computer"

export function channelPlaceKind(place: ChannelPlace): ChannelPlaceKind {
  return place === "local" || place === "boat" || place === "pool" ? place : "computer"
}

/**
 * A channel place in a person's words: Local, Boat, Any computer, or the
 * computer as the client names a place (an SSH host as itself, a paired
 * computer by the start of its key). `name` is a better name for that
 * computer when the caller has one, such as its hostname from Devices.
 */
export function channelPlaceWords(place: ChannelPlace, name?: string): string {
  switch (channelPlaceKind(place)) {
    case "local":
      return "Local"
    case "boat":
      return "Boat"
    case "pool":
      return "Any computer"
    case "computer":
      return name ?? environmentPlace(place)
  }
}
