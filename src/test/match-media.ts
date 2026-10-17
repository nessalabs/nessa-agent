/*
 * jsdom has no `matchMedia`. The kit's components read it for reduced
 * motion, so any test that draws one (a finished turn's activity line, a
 * loader) would throw. A test that needs a query to match installs its own.
 */
if (typeof window !== "undefined")
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia
