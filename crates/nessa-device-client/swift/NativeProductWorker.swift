import Foundation

/// Independent blocking lanes for an actual native product child. Rust owns
/// limits, ticket IDs, deadlines and protocol decisions. Captures remain off UI
/// while their native handles still retain count/byte permits.
public final class NativeProductWorker: @unchecked Sendable {
    private let session: NativeProductSession
    private let lock = NSLock()
    private let calls = DispatchGroup()
    private enum Lane: Hashable, Sendable { case ready, events, submit }
    private var admitted: Set<Lane> = []
    private var closing: Task<Void, Error>?
    private let submissions = DispatchQueue(label: "nessa.product.submit")
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
            return try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                guard closing == nil else {
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.closed)
                    return
                }
                guard admitted.insert(.submit).inserted else {
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.busy)
                    return
                }
                calls.enter()
                submissions.async { [self] in
                    do {
                        if stop.isStopped() { throw CancellationError() }
                        let ticket = try session.request(method: method, paramsJson: paramsJson)
                        release(.submit)
                        // An admitted command settles even if its Swift observer
                        // is canceled. Waiting cannot block the next submission.
                        ticketWait.async { [self, ticket] in
                            defer { calls.leave() }
                            let completion = ticket.wait()
                            do { continuation.resume(returning: try capture(completion)) }
                            catch {
                                continuation.resume(throwing: NativeProductCaptureFailure(
                                    completion: completion, underlying: error))
                            }
                        }
                    } catch {
                        release(.submit)
                        calls.leave()
                        continuation.resume(throwing: error)
                    }
                }
                lock.unlock()
            }
        } onCancel: { stop.stop() }
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
        defer { lock.unlock() }
        if let closing { return closing }
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
        return task
    }
    deinit {
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
