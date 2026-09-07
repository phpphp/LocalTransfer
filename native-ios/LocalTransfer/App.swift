import SwiftUI
import Combine
import UniformTypeIdentifiers

@main
struct LocalTransferApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(model)
        }
    }
}

/// 全局状态（身份 + 发现 + 服务 + 会话）。
/// 转发 disc 的 objectWillChange，视图观察 model 即可感知设备变化。
@MainActor
final class AppModel: ObservableObject {
    let me: DeviceInfo
    let disc: Discovery
    let server = MiniHTTPServer()
    private var api: TransferApi!
    private var cancellables = Set<AnyCancellable>()

    @Published var currentPeer: String?
    @Published var chats: [String: [ChatEntry]] = [:]
    @Published var progress: [String: MiniHTTPServer.ProgressInfo] = [:]
    @Published var pendingReq: IncomingReq?
    @Published var sendStatus: String?
    @Published var myQr: String?    // 本机二维码内容（点开显示）

    private var started = false

    init() {
        let defs = UserDefaults.standard
        let id: String
        if let saved = defs.string(forKey: "device_id") {
            id = saved
        } else {
            id = UUID().uuidString
            defs.set(id, forKey: "device_id")
        }
        let name = defs.string(forKey: "device_name") ?? "手机-\(id.prefix(4))"
        defs.set(name, forKey: "device_name")

        me = DeviceInfo(id: id, name: name, plat: "ios", port: 0)
        disc = Discovery(me: me)
        api = TransferApi(me: me)

        disc.objectWillChange
            .sink { [weak self] _ in self?.objectWillChange.send() }
            .store(in: &cancellables)
    }

    /// 启动服务 + 发现（onAppear 调用，幂等）
    func start() {
        guard !started else { return }
        started = true
        if let p = server.start(fromPort: defaultHTTPPort) {
            var info = me
            info.port = Int(p)
            server.setIdentity(info)
            wireServer()
            disc.start()
            // 恢复手动设备
            UserDefaults.standard.stringArray(forKey: "manual_peers")?
                .forEach { disc.addManual($0) }
        }
    }

    private func wireServer() {
        server.onMessage = { [weak self] peerId, _, text in
            Task { @MainActor in
                self?.chats[peerId, default: []].append(.text(outgoing: false, text))
            }
        }
        server.onIncoming = { [weak self] req in
            Task { @MainActor in self?.pendingReq = req }
        }
        server.onProgress = { [weak self] token, p in
            Task { @MainActor in self?.progress[token] = p }
        }
        server.onBatchDone = { [weak self] peerId, files in
            Task { @MainActor in
                self?.progress.removeAll()
                let title = files.count == 1
                    ? files[0].name : "\(files.count) 个文件"
                self?.chats[peerId, default: []].append(
                    .fileCard(outgoing: false, title: title,
                              size: files.reduce(0) { $0 + $1.size },
                              location: "文稿/Inbox", files: files))
            }
        }
    }

    func addManual(_ addr: String) {
        disc.addManual(addr)
        var list = UserDefaults.standard.stringArray(forKey: "manual_peers") ?? []
        if !list.contains(addr) { list.append(addr) }
        UserDefaults.standard.set(list, forKey: "manual_peers")
    }

    func sendText(_ text: String) {
        guard let peerId = currentPeer, let peer = disc.peers[peerId] else { return }
        chats[peerId, default: []].append(.text(outgoing: true, text))
        Task {
            do { try await api.sendText(peer: peer, text: text) }
            catch { sendStatus = error.localizedDescription }
        }
    }

    func sendPicked(_ urls: [URL]) {
        guard let peerId = currentPeer, let peer = disc.peers[peerId] else { return }
        var metas: [(FileMeta, URL)] = []
        urls.forEach { url in
            let size = Int64((try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
            metas.append((FileMeta(id: UUID().uuidString,
                                   name: url.lastPathComponent,
                                   relPath: url.lastPathComponent,
                                   size: size), url))
        }
        guard !metas.isEmpty else { return }
        chats[peerId, default: []].append(
            .fileCard(outgoing: true,
                      title: metas.count == 1 ? metas[0].0.name : "\(metas.count) 个文件",
                      size: metas.reduce(0) { $0 + $1.0.size },
                      location: nil, files: []))
        sendStatus = "发送中…"
        Task {
            do {
                try await api.sendFiles(peer: peer, files: metas) { [weak self] i, t, total in
                    Task { @MainActor in
                        self?.sendStatus =
                            "发送 \(i + 1)/\(metas.count)：\(t * 100 / max(total, 1))%"
                    }
                }
                sendStatus = nil
            } catch { sendStatus = error.localizedDescription }
        }
    }
}

// MARK: - 视图

struct RootView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        Group {
            if let peerId = model.currentPeer {
                ChatView(peerId: peerId)
            } else {
                DeviceListView()
            }
        }
        .onAppear { model.start() }
        .alert(item: $model.pendingReq) { req in
            Alert(
                title: Text("\(req.peer.name) 想发送文件"),
                message: Text("\(req.files.count) 个文件 · " +
                              fmtSize(req.files.reduce(0) { $0 + $1.size })),
                primaryButton: .default(Text("接收")) { req.complete(true) },
                secondaryButton: .cancel(Text("拒绝")) { req.complete(false) })
        }
    }
}

struct DeviceListView: View {
    @EnvironmentObject var model: AppModel
    @State private var showAdd = false
    @State private var manualAddr = ""

    private var online: [Peer] {
        model.disc.peers.values.filter(\.online)
            .sorted { $0.info.name < $1.info.name }
    }

    var body: some View {
        NavigationStack {
            List {
                if online.isEmpty {
                    Text("等待设备上线…").foregroundColor(.secondary)
                }
                ForEach(online) { p in
                    Button {
                        model.currentPeer = p.info.id
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            HStack {
                                Circle().fill(Color.green).frame(width: 6, height: 6)
                                Text(p.info.name)
                            }
                            Text("\(p.info.plat) · \(p.addr):\(p.info.port)")
                                .font(.caption).foregroundColor(.secondary)
                        }
                    }
                    .tint(.primary)
                }
            }
            .navigationTitle(model.me.name)
            .toolbar {
                ToolbarItem(placement: .navigationBarTrailing) {
                    Button { showAdd = true } label: { Image(systemName: "plus") }
                }
            }
            .sheet(isPresented: $showAdd) {
                VStack(spacing: 12) {
                    Text("添加设备").font(.headline)
                    TextField("192.168.1.5 或 192.168.1.5:17878", text: $manualAddr)
                        .textFieldStyle(.roundedBorder)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    Button("连接") {
                        model.addManual(manualAddr)
                        showAdd = false
                    }
                    .buttonStyle(.borderedProminent)
                }
                .padding()
            }
        }
    }
}

struct ChatView: View {
    @EnvironmentObject var model: AppModel
    let peerId: String
    @State private var input = ""
    @State private var showPicker = false

    private var activeProgress: [MiniHTTPServer.ProgressInfo] {
        model.progress.values.filter { $0.peerId == peerId }
    }

    var body: some View {
        VStack(spacing: 0) {
            if let s = model.sendStatus {
                Text(s).font(.caption).frame(maxWidth: .infinity)
                    .padding(.vertical, 4).background(.fill.opacity(0.5))
            }
            ScrollView {
                LazyVStack(spacing: 6) {
                    ForEach(model.chats[peerId] ?? []) { e in
                        Bubble(entry: e)
                    }
                    ForEach(activeProgress, id: \.label) { p in
                        Bubble(entry: .progress(label: p.label, idx: p.fileIdx,
                                                count: p.fileCount,
                                                transferred: p.transferred,
                                                total: p.total))
                    }
                }
                .padding()
            }
            HStack {
                Button { showPicker = true } label: { Image(systemName: "paperclip") }
                    .buttonStyle(.bordered)
                TextField("输入消息", text: $input, axis: .vertical)
                    .textFieldStyle(.roundedBorder).lineLimit(1...4)
                    .onSubmit(send)
                Button("发送", action: send).buttonStyle(.borderedProminent)
            }
            .padding(8)
        }
        .navigationTitle(model.disc.peers[peerId]?.info.name ?? "设备")
        .navigationBarTitleDisplayMode(.inline)
        .fileImporter(isPresented: $showPicker, allowedContentTypes: [.data],
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { model.sendPicked(urls) }
        }
    }

    private func send() {
        let t = input.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty else { return }
        input = ""
        model.sendText(t)
    }
}

struct Bubble: View {
    let entry: ChatEntry

    var body: some View {
        HStack {
            if entry.outgoing { Spacer(minLength: 40) }
            content
                .padding(10)
                .background(entry.outgoing ? Color.indigo
                                           : Color(.secondarySystemBackground),
                            in: RoundedRectangle(cornerRadius: 14))
            if !entry.outgoing { Spacer(minLength: 40) }
        }
    }

    @ViewBuilder private var content: some View {
        switch entry {
        case .text(_, let t):
            Text(t).foregroundColor(entry.outgoing ? .white : .primary)
        case .fileCard(_, let title, let size, let location, let files):
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Image(systemName: files.count > 1 || files.isEmpty
                          ? "folder" : "doc")
                    Text(title).lineLimit(1)
                }
                if let loc = location {
                    Text(loc).font(.caption2).foregroundColor(.secondary)
                }
                Text("\(fmtSize(size)) · 点击打开")
                    .font(.caption).foregroundColor(.secondary)
                // 多文件：应用内清单（不拉起文件 App）
                if files.count > 1 {
                    ForEach(files.prefix(30)) { f in
                        Button {
                            if let url = f.url { UIApplication.shared.open(url) }
                        } label: {
                            HStack {
                                Image(systemName: "doc")
                                Text(f.name).lineLimit(1).font(.caption)
                                Spacer()
                                Text(fmtSize(f.size)).font(.caption2)
                                    .foregroundColor(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        case .progress(let label, let idx, let count, let transferred, let total):
            VStack(alignment: .leading, spacing: 4) {
                Text("📁 \(label)（\(idx + 1)/\(count)）").font(.caption)
                Text("接收中 \(total > 0 ? transferred * 100 / total : 0)% · " +
                     "\(fmtSize(transferred)) / \(fmtSize(total))")
                    .font(.caption2).foregroundColor(.secondary)
                ProgressView(value: total > 0 ? Double(transferred) / Double(total) : 0)
            }
        }
    }
}
