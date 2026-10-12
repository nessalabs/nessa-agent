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
        case closing(Task<Void, Error>)
        case closed(Result<Void, Error>)
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

    public func enroll(code: String) async throws -> NativeEnrollmentReceipt {
        try await perform { session, _ in try session.enroll(code: code) }
    }
    public func retry(code: String) async throws -> NativeRetry {
        try await perform { session, _ in try session.retry(code: code) }
    }
    public func status() async throws -> NativeStatus {
        try await perform { session, _ in try session.status() }
    }
    public func refresh() async throws -> NativeRefresh {
        try await perform { session, stop in try session.refresh(stop: stop) }
    }
    public func cataloguePage(after: NativeEntryKey?, maximum: UInt32) async throws -> NativeCataloguePage? {
        try await perform { session, _ in try session.cataloguePage(after: after, maximum: maximum) }
    }
    public func transcript(conversation: String, revision: String) async throws -> NativeTranscript {
        try await perform { session, _ in try session.transcript(conversation: conversation, revision: revision) }
    }
    public func purgeReceipt(receiver: String) async throws -> NativePurge? {
        try await perform { session, _ in try session.purgeReceipt(receiver: receiver) }
    }

    /// Fence dispatch synchronously, signal the native owner, then share one
    /// physical drain task. Awaiting close is cancellation-independent; replacing
    /// the private scope requires a successful result, not merely starting close.
    public func close() async throws {
        try await beginClose().value
    }

    func beginClose() -> Task<Void, Error> {
        lock.lock()
        defer { lock.unlock() }
        switch state {
        case .closing(let task): return task
        case .closed(let result): return Task { try result.get() }
        case .idle, .working:
            let signal = Result { try session.requestClose() }
            let task = Task.detached { [self] in
                let result: Result<Void, Error> = await withCheckedContinuation { continuation in
                    queue.async { [self] in
                        let drain = Result { try session.close() }
                        // Preserve an earlier signal failure even if drain worked.
                        let result = signal.flatMap { drain }
                        lock.lock()
                        state = .closed(result)
                        lock.unlock()
                        continuation.resume(returning: result)
                    }
                }
                try result.get()
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
