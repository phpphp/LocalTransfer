import Foundation

let protocolVersion = 1
let discoveryGroup = "239.192.71.82"
let discoveryPort: UInt16 = 17878
let defaultHTTPPort: UInt16 = 17878
let deviceTimeoutMs: Int = 15_000
let prepareTimeoutSecs: Double = 60

struct DeviceInfo: Codable, Equatable {
    var id: String
    var name: String
    var plat: String
    var port: Int
    var v: Int

    func announceJSON() -> Data {
        var o = announceDict()
        o["t"] = "announce"
        return try! JSONSerialization.data(withJSONObject: o)
    }
    func json() -> Data {
        try! JSONSerialization.data(withJSONObject: announceDict())
    }
    private func announceDict() -> [String: Any] {
        ["id": id, "name": name, "plat": plat, "port": port, "v": v]
    }

    static func from(_ data: Data) -> DeviceInfo? {
        guard let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = j["id"] as? String else { return nil }
        return DeviceInfo(
            id: id,
            name: j["name"] as? String ?? "?",
            plat: j["plat"] as? String ?? "unknown",
            port: j["port"] as? Int ?? Int(defaultHTTPPort),
            v: j["v"] as? Int ?? 0)
    }
}

struct FileMeta: Codable {
    var id: String
    var name: String
    var relPath: String
    var size: Int64
    enum CodingKeys: String, CodingKey { case id, name, relPath = "rel_path", size }
}

struct ReceivedFile: Identifiable {
    var id = UUID()
    var name: String
    var size: Int64
    var url: URL?      // files App 里的最终位置
}

enum ChatEntry: Identifiable {
    case text(outgoing: Bool, String)
    case fileCard(outgoing: Bool, title: String, size: Int64, location: String?, files: [ReceivedFile])
    case progress(label: String, idx: Int, count: Int, transferred: Int64, total: Int64)

    var id: UUID { UUID() }
    var outgoing: Bool {
        switch self {
        case .text(let o, _): return o
        case .fileCard(let o, _, _, _, _): return o
        case .progress: return false
        }
    }
}

func fmtSize(_ bytes: Int64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"]
    var v = Double(bytes); var i = 0
    while v >= 1024 && i < 4 { v /= 1024; i += 1 }
    return i == 0 ? "\(bytes) B" : String(format: "%.1f %@", v, u[i])
}
