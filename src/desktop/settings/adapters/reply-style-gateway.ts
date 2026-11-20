import type {
  ReplyStyleApi,
  ReplyStyleChoice,
  ReplyStyleRole,
  ReplyStyles,
} from "@nessa/client"

/** How agents talk, as Settings reads and saves it. */
export interface ReplyStyleGateway {
  read(channelId?: string): Promise<ReplyStyles>
  set(
    role: ReplyStyleRole,
    choice: ReplyStyleChoice | undefined,
    channelId?: string,
  ): Promise<ReplyStyles>
}

export function replyStyleGateway(
  connected: () => Promise<{ readonly replyStyle: ReplyStyleApi }>,
): ReplyStyleGateway {
  return {
    read: async (channelId) => (await connected()).replyStyle.read(channelId),
    set: async (role, choice, channelId) =>
      (await connected()).replyStyle.set(role, choice, channelId),
  }
}
