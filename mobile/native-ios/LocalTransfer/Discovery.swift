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
}

/// 发现协议（与桌面端/Flutter/Android 版一致）：
/// UDP 多播收发 + 单播互回（2s 节流）+ 网段扫描 + TCP 保活。
/// iOS 15+ 的 Network.framework 支持本地网络多播（需 Info.plist 声明）。
final class Discovery: ObservableObject {
    @Published var peers: [String: Peer] = [:]

    private let me: DeviceInfo
    private var groupConn: NWConnection?
    private let queue = DispatchQueue(label: "lt.discovery")
    private var lastReply: [String: Date] = [:]
    private var heartbeat: Timer?
    private var scanThread: Thread?

    init(me: DeviceInfo) { self.me = me }

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

    private func multicastParams() -> NWParameters? {
        let udp = NWProtocolUDP.Options()
        guard let group = try? NWMulticastGroup(for: IPv4Address(discoveryGroup)!) else {
            return nil
        }
        udp.requiredMulticastGroups = [group]
        let params = NWParameters.udp
        params.defaultProtocolStack.transportProtocols.insert(udp, at: 0)
        params.allowLocalEndpointReuse = true
        return params
    }

    private func startMulticast() {
        guard let params = multicastParams(),
              let group = try? NWMulticastGroup(for: IPv4Address(discoveryGroup)!)
        else { return }
        let c = NWConnection(to: .multicast(group), using: params)
        groupConn = c
        c.stateUpdateHandler = { [weak self] state in
            if case .ready = state { self?.receiveLoop(on: c) }
        }
        c.start(queue: queue)
    }

    private func receiveLoop(on c: NWConnection) {
        c.receiveMessage { [weak self] data, _, _, error in
            defer { self?.receiveLoop(on: c) }
            guard let d = data, error == nil else { return }
            self?.handle(d, from: c)
        }
    }

    private func handle(_ data: Data, from c: NWConnection) {
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
                c.send(content: me.announceJSON(),
                       completion: .contentProcessed { _ in })
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
        guard let params = multicastParams(),
              let group = try? NWMulticastGroup(for: IPv4Address(discoveryGroup)!)
        else { return }
        let payload = me.announceJSON()
        // 独立短连接发送（发送组播与接收连接分离，行为最稳）
        let c = NWConnection(to: .multicast(group), using: params)
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
        // 借 announce 通道发 bye
        if let params = multicastParams(),
           let group = try? NWMulticastGroup(for: IPv4Address(discoveryGroup)!) {
            let c = NWConnection(to: .multicast(group), using: params)
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
