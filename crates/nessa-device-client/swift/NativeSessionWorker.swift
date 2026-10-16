import Foundation

/// Swift dispatch failures; native failures keep their generated typed payloads.
public enum NativeWorkerError: Error, Sendable {
    case busy
    case closed
}

/// Dispatch adapter for the synchronous generated API. The lock owns admission;
/// the supplied serial queue owns physical calls and final native deallocation.
/// `check-worker.swift` covers cancellation, queued work and close/drain ordering.
public final class NativeSessionWorker: @unchecked Sendable {
    private enum State {
        case idle
        case working
        case closing(Task<CloseResult, Never>)
        case closed(CloseResult)
    }
    private struct CloseResult: Sendable {
        let observation: NativeCloseObservation
        let signalFailure: (any Error)?
    }
    private let lock = NSLock()
    private var state = State.idle
    private let session: NativeDeviceSession
    private let queue: DispatchQueue

    private init(session: NativeDeviceSession, queue: DispatchQueue) {
        self.session = session
        self.queue = queue
    }

    /// Compose a session off the caller's executor. Cancellation before this call
    /// refuses construction; once queued, await its result and close its owner.
    public static func open(
        root: String, directory: String, address: String
    ) async throws -> NativeSessionWorker {
        try await open(root: root, directory: directory, address: address,
                       queue: DispatchQueue(label: "nessa.native.session"))
    }

    // Queue substitution stays inside this adapter/its verification module;
    // consuming UI cannot target physical native calls at the main queue.
    static func open(
        root: String, directory: String, address: String, queue: DispatchQueue
    ) async throws -> NativeSessionWorker {
        try Task.checkCancellation()
        let workerQueue = DispatchQueue(label: "nessa.native.session.worker", target: queue)
        return try await withCheckedThrowingContinuation { continuation in
            workerQueue.async {
                do {
                    let session = try NativeDeviceSession.open(
                        root: root, directory: directory, address: address)
                    continuation.resume(returning: NativeSessionWorker(session: session, queue: workerQueue))
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    // Internal so the verification executable can hold an actual admitted call.
    // Public methods expose neither a native handle nor mutable authority inputs.
    func perform<Value: Sendable>(
        _ operation: @escaping @Sendable (NativeDeviceSession, NativeReadStop) throws -> Value
    ) async throws -> Value {
        let stop = NativeReadStop()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                switch state {
                case .working:
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.busy)
                case .closing, .closed:
                    lock.unlock()
                    continuation.resume(throwing: NativeWorkerError.closed)
                case .idle:
                    state = .working
                    // Enqueue under the admission lock, so close cannot overtake
                    // an admitted call while leaving it queued after the drain.
                    queue.async { [self] in
                        let result = Result {
                            if stop.isStopped() { throw CancellationError() }
                            return try operation(session, stop)
                        }
                        lock.lock()
                        if case .working = state { state = .idle }
                        lock.unlock()
                        continuation.resume(with: result)
                    }
                    lock.unlock()
                }
            }
        } onCancel: {
            // A canceled waiter still awaits physical completion. Its stop is
            // private to this admission, including cancellation before enqueue.
            stop.stop()
        }
    }

    // Capture receives only immutable results, never the session or its stop.
    // Keep bounded passive translation in this admission: close/drain and Busy
    // cover serialization and decoding as well as the underlying native call.
    // Captures must not synchronously wait for another operation on this owner.
    public func enroll(code: String) async throws -> NativeEnrollmentReceipt {
        try await enroll(code: code, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func enroll<Value: Sendable>(
        code: String, capture: @escaping @Sendable (NativeEnrollmentReceipt) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.enroll(code: code))
        }
    }
    public func retry(code: String) async throws -> NativeRetry {
        try await retry(code: code, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func retry<Value: Sendable>(
        code: String, capture: @escaping @Sendable (NativeRetry) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.retry(code: code))
        }
    }
    public func status() async throws -> NativeStatus {
        try await status(capture: { $0 })
    }
    public func localEnrollment() async throws -> NativeLocalEnrollment? {
        try await localEnrollment(capture: { $0 })
    }
    /// Capture locally saved public correlation off UI before releasing admission.
    public func localEnrollment<Value: Sendable>(
        capture: @escaping @Sendable (NativeLocalEnrollment?) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.localEnrollment())
        }
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func status<Value: Sendable>(
        capture: @escaping @Sendable (NativeStatus) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.status())
        }
    }
    public func follow(
        conversation: String, afterIncarnation: String?, afterPosition: String?
    ) async throws -> NativeFollow {
        try await follow(
            conversation: conversation, afterIncarnation: afterIncarnation,
            afterPosition: afterPosition, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func follow<Value: Sendable>(
        conversation: String, afterIncarnation: String?, afterPosition: String?,
        capture: @escaping @Sendable (NativeFollow) throws -> Value
    ) async throws -> Value {
        try await perform { session, stop in
            try capture(
                try session.follow(
                    conversation: conversation, afterIncarnation: afterIncarnation,
                    afterPosition: afterPosition, stop: stop))
        }
    }
    public func refresh() async throws -> NativeRefresh {
        try await refresh(capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func refresh<Value: Sendable>(
        capture: @escaping @Sendable (NativeRefresh) throws -> Value
    ) async throws -> Value {
        try await perform { session, stop in
            try capture(try session.refresh(stop: stop))
        }
    }
    public func cataloguePage(after: NativeEntryKey?, maximum: UInt32) async throws -> NativeCataloguePage? {
        try await cataloguePage(after: after, maximum: maximum, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func cataloguePage<Value: Sendable>(
        after: NativeEntryKey?, maximum: UInt32, capture: @escaping @Sendable (NativeCataloguePage?) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.cataloguePage(after: after, maximum: maximum))
        }
    }
    public func transcript(conversation: String, revision: String) async throws -> NativeTranscript {
        try await transcript(conversation: conversation, revision: revision, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func transcript<Value: Sendable>(
        conversation: String, revision: String, capture: @escaping @Sendable (NativeTranscript) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.transcript(conversation: conversation, revision: revision))
        }
    }
    public func purgeReceipt(receiver: String) async throws -> NativePurge? {
        try await purgeReceipt(receiver: receiver, capture: { $0 })
    }
    /// Capture a bounded passive value off UI before releasing admission.
    public func purgeReceipt<Value: Sendable>(
        receiver: String, capture: @escaping @Sendable (NativePurge?) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.purgeReceipt(receiver: receiver))
        }
    }

    public func prepareOwnerOriginal<Value: Sendable>(
        requestId: Data, discoveryJson: String,
        capture: @escaping @Sendable (NativeOwnerPhonePreparation) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.prepareOwnerOriginal(requestId: requestId, discoveryJson: discoveryJson))
        }
    }
    public func restoreOwnerOriginal<Value: Sendable>(
        requestId: Data, discoveryJson: String,
        capture: @escaping @Sendable (NativeOwnerPhonePreparation) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.restoreOwnerOriginal(requestId: requestId, discoveryJson: discoveryJson))
        }
    }
    public func recoverOwnerOriginal<Value: Sendable>(
        expected: NativeOwnerPhonePreparation,
        capture: @escaping @Sendable (NativeOwnerStatus) throws -> Value
    ) async throws -> Value {
        try await perform { session, _ in
            try capture(try session.recoverOwnerOriginal(expected: expected))
        }
    }
    public func openProduct(instance: String) async throws -> NativeProductWorker {
        try await perform { session, _ in
            NativeProductWorker(session: try session.openProduct(instance: instance))
        }
    }

    /// Fence dispatch synchronously and wake native I/O before the serial drain.
    /// Repeated observations share the actual owner result, including a historical
    /// failure alongside confirmed private-scope release.
    public func closeObserved<Value: Sendable>(
        capture: @escaping @Sendable (NativeCloseObservation, (any Error)?) throws -> Value
    ) async throws -> Value {
        let result = await beginObservedClose().value
        return try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result { try capture(result.observation, result.signalFailure) })
            }
        }
    }
    public func close() async throws { try await beginClose().value }
    public func requestClose() { _ = beginObservedClose() }

    func beginClose() -> Task<Void, Error> {
        let observed = beginObservedClose()
        return Task.detached {
            let result = await observed.value
            if let error = result.signalFailure { throw error }
            if let failure = result.observation.failure() { throw NativeCallError.Session(failure: failure) }
            guard result.observation.released() else { throw NativeWorkerError.closed }
        }
    }
    private func beginObservedClose() -> Task<CloseResult, Never> {
        lock.lock()
        defer { lock.unlock() }
        switch state {
        case .closing(let task): return task
        case .closed(let result): return Task { result }
        case .idle, .working:
            let signalFailure: (any Error)?
            do { try session.requestClose(); signalFailure = nil }
            catch { signalFailure = error }
            let task = Task.detached { [self] in
                await withCheckedContinuation { continuation in
                    queue.async { [self] in
                        let result = CloseResult(observation: session.closeObserved(), signalFailure: signalFailure)
                        lock.lock()
                        state = .closed(result)
                        lock.unlock()
                        continuation.resume(returning: result)
                    }
                }
            }
            state = .closing(task)
            return task
        }
    }

    deinit {
        // Best-effort fallback for an abandoned owner. The queued capture keeps
        // native private state alive until its off-UI drain/deallocation finishes.
        let session = session
        try? session.requestClose()
        queue.async { try? session.close() }
    }
}
