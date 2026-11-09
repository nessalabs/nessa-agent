import Dispatch
import Foundation

/// Stand-ins for the generated session types. This file is compiled with the
/// production worker; it is not part of the Apple package.
public final class NativeProductCompletion: @unchecked Sendable {}
public final class NativeProductReady: @unchecked Sendable {}
public final class NativeProductEventBatch: @unchecked Sendable {}
public final class NativeReadStop: @unchecked Sendable {
    private let lock = NSLock()
    private var stopped = false
    func stop() { lock.withLock { stopped = true } }
    func isStopped() -> Bool { lock.withLock { stopped } }
}
public final class NativeProductTicket: @unchecked Sendable {
    private let lock = NSLock()
    private var phase = 0
    private let release = DispatchSemaphore(value: 0)
    func wait() -> NativeProductCompletion {
        lock.withLock { phase = 1 }
        release.wait()
        lock.withLock { phase = 2 }
        return NativeProductCompletion()
    }
    var isWaiting: Bool { lock.withLock { phase == 1 } }
    var isDone: Bool { lock.withLock { phase == 2 } }
    func unblock() { release.signal() }
}
final class HandOff: @unchecked Sendable {
    private let lock = NSLock()
    private var entered = false
    private let release = DispatchSemaphore(value: 0)
    let ticket = NativeProductTicket()
    func markEntered() { lock.withLock { entered = true } }
    var isEntered: Bool { lock.withLock { entered } }
    func unblock() { release.signal() }
    func waitUntilReleased() { release.wait() }
}
public final class NativeProductSession: @unchecked Sendable {
    private let lock = NSLock()
    private var handoffs: [HandOff] = []
    let blockInRequest: Bool
    private var closeDidEnter = false
    init(blockInRequest: Bool = false) { self.blockInRequest = blockInRequest }
    func waitReady() throws -> NativeProductReady { NativeProductReady() }
    func events(timeoutMs: UInt64) throws -> NativeProductEventBatch {
        _ = timeoutMs
        return NativeProductEventBatch()
    }
    func request(method: String, paramsJson: String) throws -> NativeProductTicket {
        _ = (method, paramsJson)
        let handoff = HandOff()
        lock.lock()
        handoffs.append(handoff)
        lock.unlock()
        if blockInRequest {
            handoff.markEntered()
            handoff.waitUntilReleased()
        }
        return handoff.ticket
    }
    func requestClose() {}
    func close() throws { lock.withLock { closeDidEnter = true } }
    var count: Int { lock.withLock { handoffs.count } }
    var closeEntered: Bool { lock.withLock { closeDidEnter } }
    func handoff(_ index: Int) -> HandOff {
        lock.lock()
        defer { lock.unlock() }
        return handoffs[index]
    }
}

private struct CaptureBroke: Error, Sendable {}

private enum Outcome: Equatable, Sendable {
    case value
    case capture
    case closed
    case other(String)
}

private final class Results: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Outcome] = []
    func add(_ value: Outcome) { lock.withLock { values.append(value) } }
    var count: Int { lock.withLock { values.count } }
    var all: [Outcome] { lock.withLock { values } }
}

private final class Box<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T?
    func set(_ value: T) { lock.withLock { self.value = value } }
    func get() -> T? { lock.withLock { value } }
    func claim(_ value: T) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if self.value != nil { return false }
        self.value = value
        return true
    }
}

private func poll(_ ready: @escaping @Sendable () -> Bool) async -> Bool {
    let start = Date()
    while !ready() {
        if Date().timeIntervalSince(start) > 5 { return false }
        try? await Task.sleep(nanoseconds: 10_000_000)
    }
    return true
}

private func pause() async {
    try? await Task.sleep(nanoseconds: 200_000_000)
}

@main struct CheckProductRequest {
    static func main() async {
        guard await throwingCapture(1) else { exit(1) }
        guard await throwingCapture(3) else { exit(1) }
        guard await closeDuringStartedHandOffDeliversTheTicket() else { exit(1) }
        print("PASS: a throwing capture leaves the drain once; a started hand-off still delivers its ticket")
    }

    static func throwingCapture(_ count: Int) async -> Bool {
        let session = NativeProductSession()
        let worker = NativeProductWorker(session: session)
        let results = Results()
        for _ in 0..<count {
            Task {
                do {
                    _ = try await worker.request(method: "read", paramsJson: "{}") { _ in throw CaptureBroke() }
                    results.add(.value)
                } catch is NativeProductCaptureFailure {
                    results.add(.capture)
                } catch NativeWorkerError.closed {
                    results.add(.closed)
                } catch {
                    results.add(.other(String(describing: error)))
                }
            }
        }
        guard await poll({ session.count >= count }) else { return fail("\(count) hand-offs did not start") }
        for index in 0..<count {
            guard await poll({ session.handoff(index).ticket.isWaiting }) else {
                return fail("ticket \(index) of \(count) did not reach its wait")
            }
        }
        let drained = Box<Bool>()
        let closing = Task {
            try await worker.close()
            drained.set(true)
        }
        guard await poll({ session.closeEntered }) else { return fail("close did not reach the drain") }
        await pause()
        guard drained.get() == nil else { return fail("drain finished with \(count) ticket waits still held") }
        for index in 0..<count {
            session.handoff(index).ticket.unblock()
            if index + 1 < count {
                await pause()
                guard drained.get() == nil else {
                    return fail("drain finished with \(count - index - 1) of \(count) requests still outstanding")
                }
            }
        }
        guard await poll({ results.count == count }) else {
            return fail("only \(results.count) of \(count) captures finished: \(results.all)")
        }
        guard results.all.allSatisfy({ $0 == .capture }) else {
            return fail("captures finished as \(results.all)")
        }
        guard await poll({ drained.get() == true }) else { return fail("drain did not finish after the last capture") }
        _ = closing
        return true
    }

    static func closeDuringStartedHandOffDeliversTheTicket() async -> Bool {
        let session = NativeProductSession(blockInRequest: true)
        let worker = NativeProductWorker(session: session)
        let delivered = Box<Outcome>()
        let request = Task {
            do {
                let value = try await worker.request(method: "read", paramsJson: "{}") { _ in 7 }
                delivered.set(value == 7 ? .value : .other(String(value)))
            } catch NativeWorkerError.closed {
                delivered.set(.closed)
            } catch {
                delivered.set(.other(String(describing: error)))
            }
        }
        guard await poll({ session.count >= 1 }) else { return fail("hand-off did not start") }
        guard await poll({ session.handoff(0).isEntered }) else { return fail("native call did not start") }
        let drained = Box<Bool>()
        let closing = Task {
            try await worker.close()
            drained.set(true)
        }
        guard await poll({ session.closeEntered }) else { return fail("close did not reach the drain") }
        request.cancel()
        await pause()
        guard drained.get() == nil else { return fail("drain finished while the native call was still running") }
        session.handoff(0).unblock()
        guard await poll({ session.handoff(0).ticket.isWaiting }) else {
            return fail("the ticket from a started hand-off was not waited")
        }
        await pause()
        guard drained.get() == nil else { return fail("drain finished before the ticket wait left") }
        session.handoff(0).ticket.unblock()
        guard await poll({ delivered.get() == .value }) else {
            let ticket = session.handoff(0).ticket
            return fail("the started hand-off finished as \(String(describing: delivered.get())); waiting \(ticket.isWaiting) done \(ticket.isDone)")
        }
        guard await poll({ drained.get() == true }) else { return fail("drain did not finish after the ticket wait") }
        _ = closing
        return true
    }

    static func fail(_ message: String) -> Bool {
        print("FAIL: \(message)")
        return false
    }
}
