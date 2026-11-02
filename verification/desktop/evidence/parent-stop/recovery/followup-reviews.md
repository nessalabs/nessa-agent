# Scoped follow-up reviews

MCP verifier: fresh review of immutable source SHA256 e32677f7149e99b27bd1494a1404f6ffc901e2d879dca9a9b8a640da0f13c43e against93599d3 reported no findings after the shared-marker issue was fixed. Inspected trusted target/frame/document identity, private observation plus UUID, error handling, and unchanged exact ok assertion. Listeners remain until document close; the runner owns page cleanup. No independent browser execution was claimed. Source committed as4f3d5c866.

Admission regression: independent trace confirmed the three actual lock owners order thread before admission reader, while export owns writer only. Existing five-second aggregate join cannot distinguish receipt latency from a cycle. The fixture already defines TURN_HOLD_HANG60 for loaded Windows, documenting issue558.

Review41c4 found one minor unbounded final receipt retry; fda398c1d wraps it in the same absolute completion_deadline. A fresh final review of fda398c1d against4f3d5c866 reported no findings: five-second writer bound retained; all command/provider/drain/replay/retirement waits share one existing completion ceiling; actor, receipt, single-cancel and released-lock assertions preserved; design row accurate. Production unchanged. Reviewers did not execute negative controls or certify Windows. Those executed results must be reported separately.
