import Foundation
import Network

/// 待确认的接收请求
final class IncomingReq: Identifiable {
    let id = UUID()
    let peer: DeviceInfo
    let files: [FileMeta]
    /// "存到…"选定的接收目录（nil=默认 Inbox）；UI 在 complete(true) 前设置
    var customDir: URL?
    private var answer: Bool?
    private let lock = NSLock()

    init(peer: DeviceInfo, files: [FileMeta]) {
        self.peer = peer
        self.files = files
    }

    /// UI 调用：用户点了 接收/拒绝
    func complete(_ accept: Bool) {
        lock.lock(); answer = accept; lock.unlock()
    }

    /// 非阻塞询问结果（未确认为 nil）
    func tryComplete() -> Bool? {
        lock.lock(); defer { lock.unlock() }
        return answer
    }
}

/// 接收会话（token 对应一次 prepare）
final class RecvSession {
    let token: String
    let peerId: String
    let files: [FileMeta]
    let saveURL: URL?     // "存到…"目录（nil=默认 Inbox）
    var done: [ReceivedFile] = []
    var completed = 0
    init(token: String, peerId: String, files: [FileMeta], saveURL: URL? = nil) {
        self.token = token; self.peerId = peerId; self.files = files; self.saveURL = saveURL
    }
}

/// 极简 HTTP/1.1 服务（Network.framework NWListener）。
/// 路由与桌面端 axum 对齐：info / message / prepare / upload / cancel。
final class MiniHTTPServer {
    struct ProgressInfo {
        let peerId: String, label: String
        let fileIdx: Int, fileCount: Int
        let transferred: Int64, total: Int64
    }

    var onMessage: ((String, String, String) -> Void)?          // peerId, name, text
    var onIncoming: ((IncomingReq) -> Void)?
    var onProgress: ((String, ProgressInfo) -> Void)?           // token, info
    var onBatchDone: ((String, [ReceivedFile], URL?) -> Void)?   // 含"存到…"目录（用于释放访问权限）

    private var me: DeviceInfo   // 端口确定后回填（var）
    private var listener: NWListener?
    private let queue = DispatchQueue(label: "lt.http", attributes: .concurrent)
    private var sessions: [String: RecvSession] = [:]
    private let sessionsLock = NSLock()
    private let saveDir: URL

    var port: UInt16 { listener?.port?.rawValue ?? 0 }

    init() {
        me = DeviceInfo(id: "", name: "", plat: "ios", port: 0, v: protocolVersion)
        let base = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        saveDir = base.appendingPathComponent("Inbox", isDirectory: true)
        try? FileManager.default.createDirectory(at: saveDir, withIntermediateDirectories: true)
    }

    /// 身份回填（app 启动时；info 接口返回它）
    func setIdentity(_ info: DeviceInfo) { me = info }

    func start(fromPort: UInt16 = defaultHTTPPort) -> UInt16? {
        var p = fromPort
        while p < fromPort + 20 {
            let params = NWParameters.tcp
            params.allowLocalEndpointReuse = true
            if let l = try? NWListener(using: params, on: NWEndpoint.Port(rawValue: p)!) {
                listener = l
                l.newConnectionHandler = { [weak self] conn in
                    self?.handle(conn)
                }
                l.start(queue: queue)
                return p
            }
            p += 1
        }
        return nil
    }

    func stop() { listener?.cancel() }

    // MARK: 连接处理（手写 HTTP/1.1：一问一答后关闭）

    private func handle(_ conn: NWConnection) {
        conn.start(queue: queue)
        readRequest(conn, buffer: Data()) { [weak self] method, path, headers, bodyReader in
            guard let self = self else { return }
            let result = self.route(method, path, headers, bodyReader, conn)
            self.respond(conn, status: result.0, body: result.1, json: result.2)
        }
    }

    private func readRequest(_ conn: NWConnection, buffer: Data,
                             done: @escaping (String, String, [String: String], BodyReader) -> Void) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self]
            data, _, isComplete, error in
            guard let data = data, error == nil else {
                conn.cancel(); return
            }
            var buf = buffer + data
            // 头部未读完：找 \r\n\r\n
            guard let headerEnd = buf.range(of: Data("\r\n\r\n".utf8)) else {
                if buf.count > 32 * 1024 { conn.cancel(); return }
                self?.readRequest(conn, buffer: buf, done: done)
                return
            }
            let head = String(data: buf[..<headerEnd.lowerBound], encoding: .utf8) ?? ""
            let lines = head.components(separatedBy: "\r\n")
            guard let reqLine = lines.first?.components(separatedBy: " "),
                  reqLine.count >= 2 else { conn.cancel(); return }
            var headers: [String: String] = [:]
            lines.dropFirst().forEach { l in
                let parts = l.split(separator: ":", maxSplits: 1)
                if parts.count == 2 {
                    headers[parts[0].lowercased().trimmingCharacters(in: .whitespaces)] =
                        parts[1].trimmingCharacters(in: .whitespaces)
                }
            }
            let rest = buf[headerEnd.upperBound...]
            let reader = BodyReader(conn: conn, buffer: rest,
                                    length: Int(headers["content-length"] ?? "0") ?? 0)
            done(reqLine[0], reqLine[1], headers, reader)
        }
    }

    /// 请求体读取器（流式：upload 分块回调）
    final class BodyReader {
        private let conn: NWConnection
        private var buffer: Data
        let length: Int
        private var consumed = 0

        init(conn: NWConnection, buffer: Data, length: Int) {
            self.conn = conn; self.buffer = buffer; self.length = length
        }

        /// 读满 length 字节（小块内容）
        func readAll(_ done: @escaping (Data?) -> Void) {
            if buffer.count >= length {
                done(buffer.prefix(length)); consumed = length; return
            }
            conn.receive(minimumIncompleteLength: 1,
                         maximumLength: length - buffer.count) { [weak self] data, _, _, err in
                guard let d = data, err == nil else { done(nil); return }
                self?.buffer += d
                self?.readAll(done)
            }
        }

        /// 流式读取：chunk 回调，结束后 done
        func stream(chunk: @escaping (Data) -> Void, done: @escaping (Int64) -> Void) {
            func pull(_ total: inout Int64) {
                if !buffer.isEmpty {
                    chunk(buffer); total += Int64(buffer.count)
                    consumed += buffer.count; buffer = Data()
                }
                guard consumed < length else { done(total); return }
                conn.receive(minimumIncompleteLength: 1,
                             maximumLength: 256 * 1024) { [weak self] data, _, _, err in
                    guard let d = data, err == nil, !d.isEmpty, let self = self else {
                        done(total); return
                    }
                    self.buffer = d
                    chunk(d); total += Int64(d.count); self.consumed += d.count
                    pull(&total)
                }
            }
            var t: Int64 = 0
            pull(&t)
        }
    }

    // MARK: 路由

    private func route(_ method: String, _ path: String,
                       _ headers: [String: String], _ body: BodyReader,
                       _ conn: NWConnection) -> (Int, Data, Bool) {
        switch (method, path) {
        case ("GET", "/api/info"):
            return (200, me.json(), true)

        case ("POST", "/api/message"):
            guard let bodyData = syncRead(body) else { return (400, Data(), false) }
            guard let j = try? JSONSerialization.jsonObject(with: bodyData) as? [String: Any],
                  let sender = j["sender"] as? [String: Any],
                  let sid = sender["id"] as? String,
                  let text = (j["text"] as? String)?.trimmingCharacters(in: .whitespaces),
                  !text.isEmpty, sid != me.id
            else { return (400, Data(), false) }
            let name = sender["name"] as? String ?? "?"
            DispatchQueue.main.async { self.onMessage?(sid, name, text) }
            return (200, Data(), false)

        case ("POST", "/api/transfer/prepare"):
            return prepare(body)

        default:
            if method == "PUT", path.hasPrefix("/api/transfer/upload/") {
                let seg = path.dropFirst("/api/transfer/upload/".count)
                    .split(separator: "/")
                if seg.count == 2 {
                    return upload(String(seg[0]), String(seg[1]), body, headers)
                }
            }
            if method == "POST", path.hasPrefix("/api/transfer/cancel/") {
                let token = String(path.dropFirst("/api/transfer/cancel/".count))
                sessionsLock.lock(); sessions.removeValue(forKey: token); sessionsLock.unlock()
                return (200, Data(), false)
            }
            return (404, Data(), false)
        }
    }

    private func prepare(_ body: BodyReader) -> (Int, Data, Bool) {
        guard let bodyData = syncRead(body),
              let j = try? JSONSerialization.jsonObject(with: bodyData) as? [String: Any],
              let sender = j["sender"] as? [String: Any],
              let sid = sender["id"] as? String, sid != me.id,
              let arr = j["files"] as? [[String: Any]], !arr.isEmpty
        else { return (400, Data(), false) }
        let info = DeviceInfo.from(bodyData)!

        let files: [FileMeta] = arr.compactMap { f in
            guard let id = f["id"] as? String else { return nil }
            return FileMeta(id: id,
                           name: f["name"] as? String ?? id,
                           relPath: f["rel_path"] as? String ?? "",
                           size: (f["size"] as? NSNumber)?.int64Value ?? 0)
        }
        // 等待 UI 确认（≤60s，100ms 轮询；HTTP 跑在并发队列不阻塞主线程）
        let req = IncomingReq(peer: info, files: files)
        DispatchQueue.main.async { self.onIncoming?(req) }
        let deadline = Date().addingTimeInterval(prepareTimeoutSecs)
        while Date() < deadline {
            if let ok = req.tryComplete() {
                if ok {
                    let token = UUID().uuidString
                    sessionsLock.lock()
                    sessions[token] = RecvSession(token: token, peerId: req.peer.id,
                                                  files: req.files, saveURL: req.customDir)
                    sessionsLock.unlock()
                    return (200, try! JSONSerialization.data(withJSONObject: ["token": token]), true)
                }
                // 响应体区分拒绝/超时：发送方靠"超时"关键字静默收场
                return (403, Data("对方拒绝了传输".utf8), false)
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
        return (403, Data("等待确认超时".utf8), false)
    }

    private func upload(_ token: String, _ fileId: String,
                        _ body: BodyReader, _ headers: [String: String]) -> (Int, Data, Bool) {
        sessionsLock.lock()
        guard let sess = sessions[token] else {
            sessionsLock.unlock(); return (409, Data(), false)
        }
        sessionsLock.unlock()

        let meta = sess.files.first { $0.id == fileId }
            ?? FileMeta(id: fileId, name: fileId, relPath: fileId, size: 0)
        let segs = meta.relPath.split(separator: "/").filter {
            !$0.isEmpty && $0 != "." && $0 != ".." && !$0.contains(":")
        }
        let baseDir = sess.saveURL ?? saveDir
        let fileURL = baseDir.appendingPathComponent(segs.map(String.init).joined(separator: "/"))
        try? FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: fileURL.path, contents: nil)
        let handle = try? FileHandle(forWritingTo: fileURL)

        let label = meta.relPath.contains("/")
            ? String(meta.relPath.split(separator: "/")[0]) : meta.relPath
        let total = meta.size
        let semaphore = DispatchSemaphore(value: 0)

        body.stream { chunk in
            try? handle?.write(contentsOf: chunk)
            DispatchQueue.main.async {
                self.onProgress?(token, ProgressInfo(
                    peerId: sess.peerId, label: label,
                    fileIdx: sess.completed, fileCount: sess.files.count,
                    transferred: Int64(handle?.offset() ?? 0),
                    total: total))
            }
        } done: { _ in
            try? handle?.close()
            semaphore.signal()
        }
        _ = semaphore.wait(timeout: .now() + 60)

        let size = (try? FileManager.default.attributesOfItem(atPath: fileURL.path)
            [.size] as? Int64) ?? 0
        let relClean = segs.map(String.init).joined(separator: "/")
        let received = ReceivedFile(name: meta.name, relPath: relClean,
                                    size: size, url: fileURL)

        sessionsLock.lock()
        sess.done.append(received)
        sess.completed += 1
        let finished = sess.completed >= sess.files.count
        if finished { sessions.removeValue(forKey: token) }
        sessionsLock.unlock()
        if finished {
            DispatchQueue.main.async { self.onBatchDone?(sess.peerId, sess.done, sess.saveURL) }
        }
        return (200, Data(), false)
    }

    private func syncRead(_ body: BodyReader) -> Data? {
        let semaphore = DispatchSemaphore(value: 0)
        var out: Data?
        body.readAll { d in out = d; semaphore.signal() }
        _ = semaphore.wait(timeout: .now() + 10)
        return out
    }

    private func ok200(_ token: String) -> (Int, Data, Bool) {
        (200, try! JSONSerialization.data(withJSONObject: ["token": token]), true)
    }

    private func respond(_ conn: NWConnection, status: Int, body: Data, json: Bool) {
        var head = "HTTP/1.1 \(status)\r\nContent-Length: \(body.count)\r\n"
        if json { head += "Content-Type: application/json\r\n" }
        head += "Connection: close\r\n\r\n"
        var out = Data(head.utf8); out += body
        conn.send(content: out, completion: .contentProcessed { _ in
            conn.cancel()
        })
    }
}
