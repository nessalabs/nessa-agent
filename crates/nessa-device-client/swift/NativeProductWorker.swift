import Foundation

/// Independent blocking lanes for an actual native product child. Rust owns
/// limits, ticket IDs, deadlines and protocol decisions. Captures remain off UI
/// while their native handles still retain count/byte permits.
///
/// A submission waits in `ProductSubmitLane` until the current hand-off returns.
/// The ticket wait starts only after that, on `ticketWait`, so replies stay parallel.
public final class NativeProductWorker: @unchecked Sendable {
    private let session: NativeProductSession
    private let lock = NSLock()
    private let calls = DispatchGroup()
    private enum Lane: Hashable, Sendable { case ready, events }
    private var admitted: Set<Lane> = []
    private var closing: Task<Void, Error>?
    private let submitLane = ProductSubmitLane()
    private let readyWait = DispatchQueue(label: "nessa.product.ready")
    private let eventWait = DispatchQueue(label: "nessa.product.events")
    private let ticketWait = DispatchQueue(label: "nessa.product.tickets", attributes: .concurrent)
    private let drain = DispatchQueue(label: "nessa.product.close")

    init(session: NativeProductSession) { self.session = session }

    public func ready<Value: Sendable>(
        capture: @escaping @Sendable (NativeProductReady) throws -> Value
    ) async throws -> Value {
        try await perform(on: readyWait, lane: .ready) { try capture(try $0.waitReady()) }
    }
    public func events<Value: Sendable>(
        timeoutMs: UInt64,
        capture: @escaping @Sendable (NativeProductEventBatch) throws -> Value
    ) async throws -> Value {
        try await perform(on: eventWait, lane: .events) { try capture(try $0.events(timeoutMs: timeoutMs)) }
    }
    public func request<Value: Sendable>(
        method: String, paramsJson: String,
        capture: @escaping @Sendable (NativeProductCompletion) throws -> Value
    ) async throws -> Value {
        let stop = NativeReadStop()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            // Enter before the first await so close's drain cannot miss this call.
            try admitSubmit()
            // One leave for this admission. The hand-off owns it when no ticket
            // exists. The ticket wait owns it once a ticket does, including when
            // capture throws. Those paths do not overlap.
            let ticket: NativeProductTicket
            do {
                ticket = try await submitLane.handoff { [self] () throws -> NativeProductTicket in
                    if stop.isStopped() { throw CancellationError() }
                    return try session.request(method: method, paramsJson: paramsJson)
                }
            } catch {
                calls.leave()
                throw error
            }
            // An admitted command settles even if its Swift observer is canceled.
            // This wait does not block the next hand-off.
            return try await withCheckedThrowingContinuation { continuation in
                ticketWait.async { [self, ticket] in
                    defer { calls.leave() }
                    let completion = ticket.wait()
                    do { continuation.resume(returning: try capture(completion)) }
                    catch {
                        continuation.resume(throwing: NativeProductCaptureFailure(
                            completion: completion, underlying: error))
                    }
                }
            }
        } onCancel: { stop.stop() }
    }
    private func admitSubmit() throws {
        lock.lock()
        defer { lock.unlock() }
        guard closing == nil else { throw NativeWorkerError.closed }
        calls.enter()
    }
    private func perform<Value: Sendable>(
        on queue: DispatchQueue, lane: Lane,
        operation: @escaping @Sendable (NativeProductSession) throws -> Value
    ) async throws -> Value {
        let stop = NativeReadStop()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                guard closing == nil else {
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.closed)
                    return
                }
                guard admitted.insert(lane).inserted else {
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.busy)
                    return
                }
                calls.enter()
                queue.async { [self] in
                    defer { release(lane); calls.leave() }
                    continuation.resume(with: Result {
                        if stop.isStopped() { throw CancellationError() }
                        return try operation(session)
                    })
                }
                lock.unlock()
            }
        } onCancel: { stop.stop() }
    }
    private func release(_ lane: Lane) {
        lock.withLock { _ = admitted.remove(lane) }
    }
    public func requestClose() { _ = beginClose() }
    public func close() async throws { try await beginClose().value }
    private func beginClose() -> Task<Void, Error> {
        lock.lock()
        if let closing {
            lock.unlock()
            return closing
        }
        // Wake native I/O before the drain, and before any resumed waiter runs.
        session.requestClose()
        let task = Task.detached { [self] in
            let result: Result<Void, Error> = await withCheckedContinuation { continuation in
                drain.async { [self] in
                    let result = Result { try session.close() }
                    calls.wait()
                    continuation.resume(returning: result)
                }
            }
            try result.get()
        }
        closing = task
        lock.unlock()
        // Resuming a queued caller re-enters this worker. Do not hold `lock`.
        // Waiters still in the lane fail with `closed` and leave `calls`.
        submitLane.close()
        return task
    }
    deinit {
        submitLane.close()
        let session = session
        session.requestClose()
        drain.async { try? session.close() }
    }
}

/// A capture failure cannot erase the native command's factual settlement.
public struct NativeProductCaptureFailure: Error, Sendable {
    public let completion: NativeProductCompletion
    public let underlying: any Error
}
