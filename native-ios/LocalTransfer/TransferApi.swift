import Foundation

/// 桌面端 HTTP API 客户端：发文本、prepare → 逐文件流式上传（URLSession uploadTask）。
/// URLSessionUploadTask 用文件做源，系统自动流式发送（不会反压死锁），
/// 进度经 delegate 回调。
final class TransferApi: NSObject, URLSessionTaskDelegate {
    private let me: DeviceInfo
    private var progressHandler: ((Int, Int64, Int64) -> Void)?
    private var currentIdx = 0

    init(me: DeviceInfo) { self.me = me }

    func sendText(peer: Peer, text: String) async throws {
        var req = URLRequest(url: URL(string: "\(peer.httpBase)/api/message")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: Any] = [
            "sender": ["id": me.id, "name": me.name, "plat": me.plat,
                       "port": me.port, "v": me.v],
            "text": text,
            "sent_at": Int(Date().timeIntervalSince1970 * 1000),
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (_, resp) = try await URLSession.shared.data(for: req)
        guard (resp as? HTTPURLResponse)?.statusCode == 200 else {
            throw TransferError.text("发送失败（\((resp as? HTTPURLResponse)?.statusCode ?? -1)）")
        }
    }

    /// 发一组文件：prepare（≤60s 等确认）→ 逐文件上传。
    /// [onProgress] 在后台线程回调 (fileIdx, transferred, total)。
    func sendFiles(peer: Peer, files: [(FileMeta, URL)],
                   onProgress: @escaping (Int, Int64, Int64) -> Void) async throws {
        progressHandler = onProgress

        // 1) prepare
        var req = URLRequest(url: URL(string: "\(peer.httpBase)/api/transfer/prepare")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let metas: [[String: Any]] = files.map {
            ["id": $0.0.id, "name": $0.0.name, "rel_path": $0.0.relPath,
             "size": $0.0.size]
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "sender": ["id": me.id, "name": me.name, "plat": me.plat,
                       "port": me.port, "v": me.v],
            "files": metas,
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        let code = (resp as? HTTPURLResponse)?.statusCode ?? -1
        if code == 403 {
            let msg = String(data: data, encoding: .utf8) ?? ""
            throw TransferError.text(
                msg.contains("超时") ? "等待确认超时：请在 60 秒内在电脑端点「接收」"
                                    : "对方拒绝了传输")
        }
        guard code == 200,
              let token = (try? JSONSerialization.jsonObject(with: data)
                  as? [String: Any])?["token"] as? String
        else { throw TransferError.text("对方返回 \(code)") }

        // 2) 逐文件上传
        for (i, (meta, url)) in files.enumerated() {
            currentIdx = i
            var up = URLRequest(url: URL(string:
                "\(peer.httpBase)/api/transfer/upload/\(token)/\(meta.id)")!)
            up.httpMethod = "PUT"
            up.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
            let session = URLSession(configuration: .default, delegate: self,
                                     delegateQueue: nil)
            defer { session.finishTasksAndInvalidate() }
            let (_, upResp) = try await session.upload(for: up, fromFile: url)
            let upCode = (upResp as? HTTPURLResponse)?.statusCode ?? -1
            if upCode != 200 {
                try? await URLSession.shared.data(for: URLRequest(url:
                    URL(string: "\(peer.httpBase)/api/transfer/cancel/\(token)")!)
                    .mutating { $0.httpMethod = "POST" })
                throw TransferError.text("上传失败（\(upCode)）")
            }
        }
    }

    // 进度 delegate
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    didSendBodyData bytesSent: Int64, totalBytesSent: Int64,
                    totalBytesExpectedToSend: Int64) {
        progressHandler?(currentIdx, totalBytesSent, totalBytesExpectedToSend)
    }
}

enum TransferError: LocalizedError {
    case text(String)
    var errorDescription: String? {
        switch self { case .text(let s): return s }
    }
}

private extension URLRequest {
    func mutating(_ f: (inout URLRequest) -> Void) -> URLRequest {
        var c = self; f(&c); return c
    }
}
