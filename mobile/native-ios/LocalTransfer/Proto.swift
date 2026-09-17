import Foundation

let protocolVersion = 1
let discoveryGroup = "239.192.71.82"
let discoveryPort: UInt16 = 17878
let defaultHTTPPort: UInt16 = 17878
let deviceTimeoutMs: Int = 15_000
let prepareTimeoutSecs: Double = 300

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
    var relPath: String
    var size: Int64
    var url: URL?      // files App 里的最终位置
}

/// 消息条目（struct：id 必须稳定——enum 的计算属性 id 每次访问都变，
/// ForEach 身份混乱且无法按 id 删除）
struct ChatEntry: Identifiable {
    let id = UUID()
    let kind: Kind
    private let outgoingFlag: Bool

    enum Kind {
        case text(String)
        case fileCard(title: String, size: Int64, location: String?, files: [ReceivedFile])
        case progress(label: String, idx: Int, count: Int, transferred: Int64, total: Int64)
    }

    init(outgoing: Bool, _ kind: Kind) {
        self.outgoingFlag = outgoing
        self.kind = kind
    }

    static func text(outgoing: Bool, _ t: String) -> ChatEntry {
        ChatEntry(outgoing: outgoing, .text(t))
    }
    static func fileCard(outgoing: Bool, title: String, size: Int64,
                         location: String?, files: [ReceivedFile]) -> ChatEntry {
        ChatEntry(outgoing: outgoing, .fileCard(title: title, size: size,
                                                location: location, files: files))
    }
    static func progress(label: String, idx: Int, count: Int,
                         transferred: Int64, total: Int64) -> ChatEntry {
        ChatEntry(outgoing: false, .progress(label: label, idx: idx, count: count,
                                             transferred: transferred, total: total))
    }

    var outgoing: Bool {
        if case .text = kind { return outgoingFlag }
        if case .fileCard = kind { return outgoingFlag }
        return false
    }
    var isFile: Bool {
        if case .fileCard = kind { return true }
        return false
    }
    var files: [ReceivedFile]? {
        if case .fileCard(_, _, _, let f) = kind { return f }
        return nil
    }
    var isProgress: Bool {
        if case .progress = kind { return true }
        return false
    }
    var progressLabel: String? {
        if case .progress(let l, _, _, _, _) = kind { return l }
        return nil
    }
}

func fmtSize(_ bytes: Int64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"]
    var v = Double(bytes); var i = 0
    while v >= 1024 && i < 4 { v /= 1024; i += 1 }
    return i == 0 ? "\(bytes) B" : String(format: "%.1f %@", v, u[i])
}

func fmtSpeed(_ bytesPerSec: Double) -> String {
    fmtSize(Int64(bytesPerSec)) + "/s"
}

/// 随机诗意设备名（与桌面/Android 版同词表）：形容词 + 的 + 意象
func randomPoeticName() -> String {
    let adj = ["温柔的", "安静的", "快乐的", "勇敢的", "自由的", "神秘的", "优雅的", "活泼的", "沉静的",
               "明亮的", "柔软的", "轻盈的", "悠然的", "清澈的", "可爱的", "狡黠的", "坦率的", "浪漫的",
               "顽皮的", "认真的", "热烈的", "朦胧的", "顺风的", "发光的", "微笑的", "好奇的",
               "懒洋洋的", "慢悠悠的", "亮晶晶的", "毛茸茸的", "圆滚滚的", "慢半拍的"]
    let noun = ["山雀", "鲸鱼", "萤火", "松鼠", "云雀", "海豚", "月光", "星河", "芦苇", "清泉", "晚风",
                "候鸟", "竹叶", "雪花", "灯塔", "小熊", "旅人", "橘猫", "白鹭", "远山", "湖泊", "松果",
                "蒲公英", "布谷鸟", "小雨滴", "贝壳", "枫叶", "流星", "麦浪", "溪水", "云朵", "海风"]
    return adj.randomElement()! + noun.randomElement()!
}
