# GenAI traces

A Claude, Codex, or OpenCode turn records OpenTelemetry GenAI spans from the
session updates and permission events the worker already decoded. The catalog
of `nessa.*` attributes, and the order a turn's spans open and close, is
[GenAI traces](../../../../docs/design/agent_execution/gen-ai-traces.md).

`Agent::read_traces` returns the finished spans for a conversation. Content
capture is off unless the provider's store was built with it on. A later OTLP
sender is a `SpanExporter` installed on that store; this slice does not open
one.
