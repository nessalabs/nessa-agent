/// Swift dispatch failures. Native calls keep their generated typed payloads.
public enum NativeWorkerError: Error, Sendable {
    /// A session call, or a product Ready or event wait, is already admitted.
    /// A product submission does not use this case. It waits for the hand-off.
    case busy
    /// This worker is closing or has closed.
    case closed
}
