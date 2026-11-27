import CLnaFfi
import Foundation

public struct ApiEvent: Sendable {
    public let seq: UInt64
    public let topic: String
    public let event: String
    public let data: [String: Any]
}

/// An error answered by the core: a stable `code` (for localized advice) plus the raw message.
public struct CoreError: LocalizedError {
    public let code: String
    public let message: String
    public var errorDescription: String? { message }
}

public final class CoreClient {
    private var core: OpaquePointer?
    private let lock = NSLock()
    private var pending: [String: CheckedContinuation<[String: Any], Error>] = [:]
    public var onEvent: ((ApiEvent) -> Void)?

    public init() {}

    public func open(dataDir: String) throws {
        let cfgObj: [String: Any] = ["data_dir": dataDir, "allow_memory_secret_fallback": false]
        let cfgData = try JSONSerialization.data(withJSONObject: cfgObj)
        let cfgStr = String(data: cfgData, encoding: .utf8) ?? "{}"

        var outCore: OpaquePointer?
        var outErr: UnsafeMutablePointer<CChar>?
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        let rc = cfgStr.withCString { cstr in
            lna_open(cstr, { jsonPtr, len, user in
                guard let jsonPtr, let user else { return }
                let client = Unmanaged<CoreClient>.fromOpaque(user).takeUnretainedValue()
                let data = Data(bytes: jsonPtr, count: len)
                if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                    let ev = ApiEvent(
                        seq: obj["seq"] as? UInt64 ?? 0,
                        topic: obj["topic"] as? String ?? "",
                        event: obj["event"] as? String ?? "",
                        data: obj["data"] as? [String: Any] ?? [:]
                    )
                    DispatchQueue.main.async { client.onEvent?(ev) }
                }
            }, selfPtr, &outCore, &outErr)
        }

        if rc != 0 {
            let msg = outErr.map { String(cString: $0) } ?? L10n.t("core.open_failed", en: L10n.systemEnglish)
            if let outErr { lna_string_free(outErr) }
            throw NSError(domain: "LocalNotebook", code: Int(rc), userInfo: [NSLocalizedDescriptionKey: msg])
        }
        self.core = outCore
    }

    public func call(method: String, params: [String: Any] = [:]) async throws -> [String: Any] {
        guard let core else {
            throw CoreError(code: "NOT_READY", message: "Core not initialized")
        }
        let reqId = UUID().uuidString
        let reqObj: [String: Any] = ["id": reqId, "method": method, "params": params]
        let reqData = try JSONSerialization.data(withJSONObject: reqObj)
        let reqStr = String(data: reqData, encoding: .utf8) ?? "{}"
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        return try await withCheckedThrowingContinuation { cont in
            lock.lock()
            pending[reqId] = cont
            lock.unlock()

            reqStr.withCString { cstr in
                lna_call(core, cstr, { jsonPtr, len, user in
                    guard let jsonPtr, let user else { return }
                    let client = Unmanaged<CoreClient>.fromOpaque(user).takeUnretainedValue()
                    let data = Data(bytes: jsonPtr, count: len)
                    guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                          let id = obj["id"] as? String else { return }

                    client.lock.lock()
                    let continuation = client.pending.removeValue(forKey: id)
                    client.lock.unlock()

                    if let err = obj["error"] as? [String: Any] {
                        let msg = err["message"] as? String ?? L10n.t("core.unknown_error", en: L10n.systemEnglish)
                        continuation?.resume(throwing: CoreError(code: err["code"] as? String ?? "", message: msg))
                    } else {
                        continuation?.resume(returning: obj["result"] as? [String: Any] ?? [:])
                    }
                }, selfPtr)
            }
        }
    }

    public func close() {
        if let core {
            lna_close(core)
            self.core = nil
        }
    }

    deinit { close() }
}
