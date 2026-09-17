import Foundation
import Network

/// 对端设备
struct Peer: Identifiable {
    let info: DeviceInfo
    let addr: String
    var manual: Bool
    var lastSeen: Date
    var id: String { info.id }
    var online: Bool { manual || Date().timeIntervalSince(lastSeen) < 15 }
    /// HTTP 基址（TransferApi 用）
    var httpBase: String { "http://\(addr):\(info.port)" }
}

/// 发现协议（与桌面端/Flutter/Android 版一致）：
/// UDP 多播收发 + 单播互回（2s 节流）+ 网段扫描 + TCP 保活。
/// iOS 15+ 的 Network.framework 支持本地网络多播（需 Info.plist 声明）。
final class Discovery: ObservableObject {
    @Published var peers: [String: Peer] = [:]

    private var me: DeviceInfo   // var：改名后下一条 announce 携带新名
    private var groupConn: NWConnectionGroup?
    private let queue = DispatchQueue(label: "lt.discovery")
    private var lastReply: [String: Date] = [:]
    private var heartbeat: Timer?
    private var scanThread: Thread?

    init(me: DeviceInfo) { self.me = me }

    /// 改名（立即生效于后续 announce 与单播回复）
    func setName(_ name: String) { me.name = name }

    func start() {
        startMulticast()
        // 心跳：每 5s announce + 每 6s 保活刷新（合并在一个 5s 定时器里）
        heartbeat = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            self?.announce()
            self?.keepalive()
        }
        announce()
        // 网段扫描：启动即扫 + 每 45s（UDP 被拦时的自动发现兜底）
        scanThread = Thread { [weak self] in
            self?.subnetScan()
            while true {
                Thread.sleep(forTimeInterval: 45)
                self?.subnetScan()
            }
        }
        scanThread?.start()
    }

    // MARK: UDP 多播

    /// iOS 的多播组用端点形式（macOS 的 IPv4Address 便捷构造在 iOS SDK 不存在）：
    /// NWMulticastGroup(for: [.host(host:port:)])，需 Info.plist 声明本地网络权限
    private func multicastGroup() -> NWMulticastGroup? {
        guard let port = NWEndpoint.Port(rawValue: discoveryPort) else { return nil }
        return try? NWMulticastGroup(for: [
            .hostPort(host: NWEndpoint.Host(discoveryGroup), port: port)
        ])
    }

    private func startMulticast() {
        // iOS 加入多播组：NWConnectionGroup(with: 组描述符, using: 参数)
        // （requiredMulticastGroups 是 macOS-only，iOS SDK 无此成员）
        guard let group = multicastGroup() else { return }
        let params = NWParameters.udp
        params.allowLocalEndpointReuse = true
        let c = NWConnectionGroup(with: group, using: params)
        groupConn = c
        c.stateUpdateHandler = { [weak self] state in
            if case .ready = state { self?.receiveLoop(on: c) }
        }
        c.start(queue: queue)
    }

    private func receiveLoop(on c: NWConnectionGroup) {
        c.setReceiveHandler(maximumMessageSize: 65536) { [weak self] message, data, _ in
            guard let d = data else { return }
            self?.handle(d, replyVia: message)
        }
    }

    private func handle(_ data: Data, replyVia message: NWConnectionGroup.Message) {
        guard let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let t = j["t"] as? String else { return }
        switch t {
        case "announce":
            guard let info = DeviceInfo.from(data),
                  info.id != me.id, info.v == protocolVersion else { return }
            let isNewOrChanged = peers[info.id]?.info != info
            DispatchQueue.main.async {
                let manual = self.peers[info.id]?.manual ?? false
                let addr = self.peers[info.id]?.addr ?? ""
                self.peers[info.id] = Peer(info: info, addr: addr, manual: manual,
                                           lastSeen: Date())
            }
            // 单播回自己（2s 节流防互回风暴）
            let now = Date()
            if now.timeIntervalSince(lastReply[info.id] ?? .distantPast) >= 2 {
                lastReply[info.id] = now
                message.reply(content: me.announceJSON())
            }
            _ = isNewOrChanged
        case "bye":
            if let id = j["id"] as? String {
                DispatchQueue.main.async { self.peers[id] = nil }
            }
        default: break
        }
    }

    private func announce() {
        guard let port = NWEndpoint.Port(rawValue: discoveryPort) else { return }
        let payload = me.announceJSON()
        // 发送：普通 host 端点直接向多播地址发 UDP（iOS 无需特殊 API）
        let c = NWConnection(to: .hostPort(host: NWEndpoint.Host(discoveryGroup), port: port),
                             using: .udp)
        c.stateUpdateHandler = { state in
            if case .ready = state {
                c.send(content: payload, completion: .contentProcessed { _ in
                    c.cancel()
                })
            }
        }
        c.start(queue: queue)
    }

    // MARK: TCP 兜底

    func subnetScan() {
        guard let own = Self.myLanIp(),
              let dot = own.lastIndex(of: ".") else { return }
        let prefix = String(own[..<dot])
        let sem = DispatchSemaphore(value: 0)
        (1...254).forEach { i in
            [defaultHTTPPort, defaultHTTPPort + 1].forEach { port in
                probe("\(prefix).\(i)", port: Int(port), addNew: true) { _ in
                    sem.signal()
                }
            }
        }
        // 等待一轮（上限 8s，超时忽略——结果异步入表即可）
        for _ in 0..<(254 * 2) {
            if sem.wait(timeout: .now() + 8) == .timedOut { break }
        }
    }

    func keepalive() {
        peers.values.forEach { p in
            guard !p.addr.isEmpty else { return }
            probe(p.addr, port: p.info.port, addNew: false) { _ in }
        }
    }

    /// HTTP 探测 /api/info；addNew=true 时未知设备也入表
    func probe(_ ip: String, port: Int, addNew: Bool,
               done: @escaping (Bool) -> Void) {
        var req = URLRequest(url: URL(string: "http://\(ip):\(port)/api/info")!)
        req.timeoutInterval = 1.5
        URLSession.shared.dataTask(with: req) { [weak self] data, resp, _ in
            guard let http = resp as? HTTPURLResponse, http.statusCode == 200,
                  let d = data, let info = DeviceInfo.from(d),
                  info.id != self?.me.id ?? "" else {
                done(false); return
            }
            let exists = self?.peers[info.id] != nil
            if addNew || exists {
                DispatchQueue.main.async {
                    let manual = self?.peers[info.id]?.manual ?? false
                    self?.peers[info.id] = Peer(info: info, addr: ip, manual: manual,
                                                lastSeen: Date())
                }
            }
            done(true)
        }.resume()
    }

    func addManual(_ host: String) {
        var ip = host.trimmingCharacters(in: .whitespaces)
            .replacingOccurrences(of: "http://", with: "")
        var port = Int(defaultHTTPPort)
        if ip.contains(":") {
            let parts = ip.split(separator: ":")
            ip = String(parts[0]); port = Int(parts[1]) ?? port
        }
        guard !ip.isEmpty else { return }
        // 手动设备默认在线（manual=true 不参与超时）
        probe(ip, port: port, addNew: true) { [weak self] ok in
            if ok {
                DispatchQueue.main.async {
                    let id = self?.peers.first(where: { $0.value.addr == ip })?.key
                    if let id = id { self?.peers[id]?.manual = true }
                }
            }
        }
    }

    func shutdown() {
        let bye = try! JSONSerialization.data(withJSONObject: ["t": "bye", "id": me.id])
        // 借 announce 通道发 bye（host 端点直发多播地址）
        if let port = NWEndpoint.Port(rawValue: discoveryPort) {
            let c = NWConnection(to: .hostPort(host: NWEndpoint.Host(discoveryGroup), port: port),
                                 using: .udp)
            c.stateUpdateHandler = { state in
                if case .ready = state {
                    c.send(content: bye, completion: .contentProcessed { _ in c.cancel() })
                }
            }
            c.start(queue: queue)
        }
        groupConn?.cancel()
        heartbeat?.invalidate()
        scanThread?.cancel()
    }

    /// getifaddrs 取第一个非环回 IPv4（en 前缀 = Wi-Fi/以太网）
    static func myLanIp() -> String? {
        var ifaddr: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifaddr) == 0 else { return nil }
        defer { freeifaddrs(ifaddr) }
        var ptr = ifaddr
        while let p = ptr {
            defer { ptr = p.pointee.ifa_next }
            let sa = p.pointee.ifa_addr
            guard sa!.pointee.sa_family == UInt8(AF_INET),
                  let name = String(validatingUTF8: p.pointee.ifa_name),
                  name.hasPrefix("en") || name.hasPrefix("bridge")
            else { continue }
            var addr = sockaddr_in()
            memcpy(&addr, sa!, MemoryLayout<sockaddr_in>.size)
            var buf = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            inet_ntop(AF_INET, &addr.sin_addr, &buf, socklen_t(INET_ADDRSTRLEN))
            let ip = String(cString: buf)
            if !ip.isEmpty && ip != "127.0.0.1" { return ip }
        }
        return nil
    }
}
