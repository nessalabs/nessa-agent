# Native publication test admission gates

Actual Windows CI at05bc failed the identical in-flight request witness. The same retained receipt counts its first caller as well as its second, so a test must wait for both before releasing its held native/settings fixture. Three existing test gates now do so; the sole-caller gate and production code are unchanged.

See [author proof](author-report.md), [typed four-stage results](semantic-results.json), [actual controlled scheduling diff](controlled-scheduling.patch), [source identity](final-source.json), [fresh independent review](independent-review.md) and [combined host check](combined-host-results.json). Temporary scheduler code is absent from final production/tests. Zero-selected auxiliary binaries and overlapping suites are not added to pass totals.

Fresh supported-platform CI is still required after this source update. [Full browser verification](../final-browser-05bc/report.md) passes at the exact recorded source; no browser executable changes here. The separate50ms interaction performance gate remains failed33/35 and prevents merge.
