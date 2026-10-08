/** The setup linger question, as the host answered it. */

import { lingerAccept, lingerStatus } from "../../host/window"
import { parseLingerView, type LingerView } from "../model/linger"

export interface LingerSource {
  status(): Promise<LingerView | undefined>
  accept(): Promise<LingerView | undefined>
}

async function asked(read: () => Promise<unknown>): Promise<LingerView | undefined> {
  return parseLingerView(await read())
}

/** The process's linger port. A browser has no logind and gets the no-op view. */
export const hostLinger: LingerSource = {
  status: () => asked(lingerStatus),
  accept: () => asked(lingerAccept),
}
