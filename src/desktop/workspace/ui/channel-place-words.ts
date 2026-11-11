/**
 * A channel place in a person's words: Local, Boat, Any computer, or the
 * computer as the client names a place (an SSH host as itself, a paired
 * computer by the start of its key). `name` is a better name for that
 * computer when the caller has one, such as its hostname from Devices.
 */
import { environmentPlace } from "@nessa/client"
import { channelPlaceKind, type ChannelPlace } from "../model/channel-place"

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
