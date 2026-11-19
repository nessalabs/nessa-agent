import { isCloud } from "../../clouds/model/clouds"

/**
 * Where a channel runs its agents, in the gateway's words (`channel.setPlacement`):
 * `local`, `pool`, a cloud by its name (`boat`, `agent37`: the clouds in
 * `clouds.ts`), or one computer by its place name, an SSH
 * destination the gateway serves or a paired computer's `peer:<key>`. Pure;
 * the gateway decides which computers a channel may name. How a person reads
 * one is the UI's `channelPlaceWords`.
 */
export type ChannelPlace = string

/** Which kind of place a channel place is: one computer for any other name. */
export type ChannelPlaceKind = "local" | "cloud" | "pool" | "computer"

export function channelPlaceKind(place: ChannelPlace): ChannelPlaceKind {
  if (place === "local" || place === "pool") return place
  return isCloud(place) ? "cloud" : "computer"
}
