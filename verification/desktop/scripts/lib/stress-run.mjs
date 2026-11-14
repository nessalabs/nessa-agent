/** A failed drain refuses the step (`stress-run.test.mjs`). */
export class BackgroundDrainTimeout extends Error {
  constructor() {
    super("background turns did not drain before the exclusive step")
    this.name = "BackgroundDrainTimeout"
  }
}

/** Everything the run shares: the stack, the page, and the client in use now. */
export class Run {
  constructor(stack, opened, knobs, restartConsole, waitForBackground) {
    this.waitForBackground = waitForBackground
    this.restartConsole = restartConsole
    this.stack = stack
    this.acpRecording = stack.acpRecording
    this.opened = opened
    this.page = opened.page
    this.knobs = knobs
    this.client = stack.client
    this.mode = "ask"
    /** Gateway restarts so far: a request in flight across one may lose its answer. */
    this.restarts = 0
    this.main = null
    /** Turns a background conversation has in flight; a restart waits for none. */
    this.busy = 0
    this.paused = null
    this.background = []
    /** Other conversations a step opened that the final rest check sweeps too. */
    this.checked = []
    /** Conversations a step deleted: followers let them go, and nothing sweeps them. */
    this.deleted = new Set()
    this.sent = new Map()
    this.owned = new Set()
    /** Where native linking listens once the `linking` step turns it on. */
    this.nativeAddress = stack.nativeAddress
    /** What config.json names for it: every interface, on the same port. */
    this.nativeListen = stack.nativeListen
  }

  /** Waits while a restart holds the gateway. */
  async enter() {
    while (this.paused) await this.paused
    this.busy += 1
  }

  leave() {
    this.busy -= 1
  }

  /** Runs `body` with no background turn in flight and none starting. */
  async exclusive(body) {
    let release
    this.paused = new Promise((resolve) => (release = resolve))
    try {
      if (!(await this.waitForBackground(() => this.busy === 0, 120_000)))
        throw new BackgroundDrainTimeout()
      return await body()
    } finally {
      this.paused = null
      release()
    }
  }
}
