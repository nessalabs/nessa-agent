import Foundation
@main struct Verify {
    static func main() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            DispatchQueue(label: "nessa.native.verification").async {
                do { try check(); continuation.resume() }
                catch { continuation.resume(throwing: error) }
            }
        }
    }
    static func check() throws {
        precondition(!Thread.isMainThread)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("nessa-native-ffi-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: root) }
        var cache: NativeSavedCache? = try NativeSavedCache.open(path: root.appendingPathComponent("retained.sqlite").path)
        let id = "00000000-0000-4000-8000-000000000001"
        let snapshot = try cache!.transcript(receiver: "receiver", origin: "origin", conversation: id, revision: id)
        precondition(snapshot.state() == .notLoaded)
        precondition(snapshot.downloaded() == nil && snapshot.applied() == nil)
        precondition(snapshot.scope() == nil && snapshot.generation() == nil)
        let encoded = try snapshot.viewJson()
        precondition(encoded == nil)
        do {
            _ = try cache!.transcript(receiver: "", origin: "origin", conversation: id, revision: id)
            fatalError("invalid target was accepted")
        } catch NativeViewError.InvalidTarget { }
        cache = nil
        precondition(snapshot.state() == .notLoaded)
        let reopened = try NativeSavedCache.open(path: root.appendingPathComponent("retained.sqlite").path)
        let next = try reopened.transcript(receiver: "receiver", origin: "origin", conversation: id, revision: id)
        precondition(next.state() == .notLoaded)
        print("PASS: generated Swift linked shared core; real cache open/query/reopen, owned snapshot after handle release, typed failure distinct from absence")
    }
}
