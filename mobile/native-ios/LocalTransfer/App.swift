import SwiftUI
import Combine
import UniformTypeIdentifiers
import UserNotifications
import CoreImage.CIFilterBuiltins
import AVFoundation

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
    private(set) var me: DeviceInfo          // 改名走 rename()（同步 disc/server）
    let disc: Discovery
    let server = MiniHTTPServer()
    private var api: TransferApi!
    private var cancellables = Set<AnyCancellable>()

    @Published var currentPeer: String?
    @Published var chats: [String: [ChatEntry]] = [:]
    @Published var progress: [String: MiniHTTPServer.ProgressInfo] = [:]
    @Published var speeds: [String: Double] = [:]           // token → B/s（400ms 采样）
    /// 待确认的接收请求队列（新请求不再顶掉旧的——单槽曾让旧请求永远无人答复）
    @Published var pendingReqs: [IncomingReq] = []
    @Published var sendStatus: String?
    @Published var isForeground = true
    @Published var showSaveDirPicker = false   // "存到..."目录选择器
    /// 应用是否在前台（仅后台发本地通知）；改名/设置用
    @AppStorage("auto_receive") var autoReceive = false
    @AppStorage("device_name_custom") var deviceNameCustom = "" // 空=跟随系统设备名

    /// 生效设备名：空 = 跟随系统设备名（与桌面/Android 一致）
    var effectiveName: String {
        let c = deviceNameCustom.trimmingCharacters(in: .whitespaces)
        return c.isEmpty ? UIDevice.current.name : c
    }

    private var speedSamples: [String: (at: Date, bytes: Int64)] = [:]
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
        me = DeviceInfo(id: id, name: "", plat: "ios", port: 0, v: protocolVersion)
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
        refreshName()
        // 本地通知授权（拒了也不影响功能）
        UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound]) { _, _ in }
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

    /// 改名（同步 disc 的 announce 身份与 HTTP info；落库）
    func rename(_ raw: String) {
        let n = raw.trimmingCharacters(in: .whitespaces)
        deviceNameCustom = n
        refreshName()
    }

    private func refreshName() {
        me.name = effectiveName
        disc.setName(me.name)
        server.setIdentity(me)
        objectWillChange.send()
    }

    private func wireServer() {
        server.onMessage = { [weak self] peerId, name, text in
            Task { @MainActor in
                self?.chats[peerId, default: []].append(.text(outgoing: false, text))
                self?.notify(name, String(text.prefix(40)))
            }
        }
        server.onIncoming = { [weak self] req in
            Task { @MainActor in
                self?.notify("\(req.peer.name) 想发送文件",
                             "\(req.files.count) 个文件 · 点击处理")
                if self?.autoReceive == true {
                    req.complete(true)   // 静默接收，不弹窗
                } else {
                    self?.pendingReqs.append(req)
                }
            }
        }
        server.onProgress = { [weak self] token, p in
            Task { @MainActor in
                guard let self = self else { return }
                self.progress[token] = p
                // 速度采样（400ms 窗口；换文件自动重置基线）
                let now = Date()
                if let last = self.speedSamples[token],
                   now.timeIntervalSince(last.at) >= 0.4 {
                    self.speeds[token] = Double(p.transferred - last.bytes)
                        / now.timeIntervalSince(last.at)
                    self.speedSamples[token] = (now, p.transferred)
                } else if self.speedSamples[token] == nil {
                    self.speedSamples[token] = (now, p.transferred)
                }
            }
        }
        server.onBatchDone = { [weak self] peerId, files, dir in
            Task { @MainActor in
                guard let self = self else { return }
                // "存到…"目录的访问权限随批次结束释放
                if let scoped = dir { scoped.stopAccessingSecurityScopedResource() }
                self.progress.removeAll()
                self.speeds.removeAll()
                self.speedSamples.removeAll()
                let title: String
                if files.count == 1 {
                    let rel = files[0].relPath
                    title = rel.contains("/") ? String(rel.split(separator: "/")[0]) : rel
                } else if let first = files.first?.relPath,
                          first.contains("/"),
                          files.allSatisfy({ $0.relPath.split(separator: "/").first
                              == first.split(separator: "/").first }) {
                    title = "\(first.split(separator: "/")[0])（\(files.count) 个文件）"
                } else {
                    title = "\(files.count) 个文件"
                }
                let loc = dir?.lastPathComponent ?? "文稿/Inbox"
                self.chats[peerId, default: []].append(
                    .fileCard(outgoing: false, title: title,
                              size: files.reduce(0) { $0 + $1.size },
                              location: loc, files: files))
                self.notify("已接收", "保存在 \(loc)")
            }
        }
    }

    // MARK: 本地通知（仅后台时发，仿 QQ/微信）

    func notify(_ title: String, _ body: String) {
        guard !isForeground else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let req = UNNotificationRequest(identifier: UUID().uuidString,
                                        content: content, trigger: nil)
        UNUserNotificationCenter.current().add(req)
    }

    // MARK: 会话操作

    func clearChat(_ peerId: String) {
        chats[peerId] = []
    }

    /// 删除一条消息（按 id；ChatEntry 现在有稳定 id）
    func deleteEntry(_ id: UUID, in peerId: String) {
        chats[peerId]?.removeAll { $0.id == id }
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

    /// 发送剪贴板文本
    func sendClipboard() {
        guard let t = UIPasteboard.general.string?
            .trimmingCharacters(in: .whitespacesAndNewlines), !t.isEmpty else {
            sendStatus = "剪贴板没有文本"
            return
        }
        sendText(t)
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
        uploadAll(peerId: peerId, peer: peer, metas: metas, title:
            metas.count == 1 ? metas[0].0.name : "\(metas.count) 个文件")
    }

    /// 发送文件夹：fileImporter 选目录 → 递归收集（rel = 文件夹名/子目录/文件）
    func sendFolder(_ url: URL) {
        guard let peerId = currentPeer, let peer = disc.peers[peerId] else { return }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let rootName = url.lastPathComponent
        var metas: [(FileMeta, URL)] = []
        if let en = FileManager.default.enumerator(
            at: url, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey]) {
            for case let f as URL in en {
                let vals = try? f.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
                guard vals?.isRegularFile == true else { continue }
                let sub = f.path.hasPrefix(url.path + "/")
                    ? String(f.path.dropFirst(url.path.count + 1)) : f.lastPathComponent
                let size = Int64(vals?.fileSize ?? 0)
                metas.append((FileMeta(id: UUID().uuidString,
                                       name: f.lastPathComponent,
                                       relPath: "\(rootName)/\(sub)",
                                       size: size), f))
            }
        }
        guard !metas.isEmpty else { sendStatus = "文件夹为空"; return }
        uploadAll(peerId: peerId, peer: peer, metas: metas, title:
            "\(rootName)（\(metas.count) 个文件）")
    }

    private func uploadAll(peerId: String, peer: Peer,
                           metas: [(FileMeta, URL)], title: String) {
        let sum = metas.reduce(0) { $0 + $1.0.size }
        let card = ChatEntry.fileCard(outgoing: true, title: title, size: sum,
                                      location: nil, files: [])
        let cardId = card.id
        chats[peerId, default: []].append(card)
        let key = "send-" + UUID().uuidString
        progress[key] = MiniHTTPServer.ProgressInfo(
            peerId: peerId, label: title, fileIdx: 0, fileCount: metas.count,
            transferred: 0, total: sum)
        Task {
            defer { progress[key] = nil; speeds[key] = nil; speedSamples[key] = nil }
            do {
                let ok = try await api.sendFiles(peer: peer, files: metas) {
                    [weak self] i, t, total in
                    Task { @MainActor in
                        self?.progress[key] = MiniHTTPServer.ProgressInfo(
                            peerId: peerId, label: title, fileIdx: i,
                            fileCount: metas.count, transferred: t, total: total)
                    }
                }
                if !ok {
                    // 等待确认超时：静默收场，撤掉预挂的发送卡（与 Android 一致）
                    deleteEntry(cardId, in: peerId)
                }
            } catch { sendStatus = error.localizedDescription }
        }
    }
}

// MARK: - 根视图

struct RootView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        ZStack {
            Group {
                if let peerId = model.currentPeer {
                    ChatView(peerId: peerId)
                } else {
                    DeviceListView()
                }
            }
            // 接收请求弹窗（队头先弹，处理完自动弹下一个）
            if let req = model.pendingReqs.first {
                Color.black.opacity(0.35).ignoresSafeArea()
                ReceiveDialog(
                    req: req,
                    queueCount: model.pendingReqs.count,
                    onReject: {
                        req.complete(false)
                        model.pendingReqs.removeAll { $0.id == req.id }
                    },
                    onAccept: {
                        model.currentPeer = req.peer.id
                        req.complete(true)
                        model.pendingReqs.removeAll { $0.id == req.id }
                    },
                    onSaveTo: {
                        model.pickSaveDir(for: req)
                    })
            }
        }
        .onAppear { model.start() }
        .onChange(of: scenePhase) { ph in
            model.isForeground = ph == .active
        }
        .sheet(isPresented: $model.showSaveDirPicker) {
            FolderPicker { url in
                model.applySaveDir(url)
            }
        }
    }
}

extension AppModel {
    private var pickingReq: IncomingReq? { pendingReqs.first }

    /// "存到…"：弹系统目录选择器（Files App 的任意位置）
    func pickSaveDir(for req: IncomingReq) {
        showSaveDirPicker = true
    }

    /// 选好目录：持有访问权限，完成队头请求（批次结束在 onBatchDone 释放）
    func applySaveDir(_ url: URL) {
        _ = url.startAccessingSecurityScopedResource()
        if let req = pickingReq {
            req.customDir = url
            currentPeer = req.peer.id
            req.complete(true)
            pendingReqs.removeAll { $0.id == req.id }
            sendStatus = "本批将保存到 \(url.lastPathComponent)"
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                self?.sendStatus = nil
            }
        }
    }
}

// MARK: - 接收弹窗（卡片式：渐变图标 + 摘要胶囊 + 拒绝/接收/存到…）

struct ReceiveDialog: View {
    let req: IncomingReq
    let queueCount: Int
    let onReject: () -> Void
    let onAccept: () -> Void
    let onSaveTo: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            // 顶部渐变图标
            RoundedRectangle(cornerRadius: 19)
                .fill(LinearGradient(colors: [.indigo, .indigo.opacity(0.65)],
                                     startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(width: 58, height: 58)
                .overlay(Image(systemName: "square.and.arrow.down")
                    .font(.system(size: 28)).foregroundColor(.white))
            Text(req.peer.name).font(.headline)
            Text("想发送文件给你").font(.subheadline).foregroundColor(.secondary)
            if queueCount > 1 {
                Text("处理完还有 \(queueCount - 1) 个请求")
                    .font(.caption2).foregroundColor(.secondary)
            }
            // 文件数 + 大小摘要胶囊
            HStack(spacing: 5) {
                Image(systemName: "doc").font(.caption)
                Text("\(req.files.count) 个文件 · " +
                      fmtSize(req.files.reduce(0) { $0 + $1.size }))
                    .font(.footnote.weight(.semibold))
            }
            .foregroundColor(.indigo)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(.indigo.opacity(0.09), in: Capsule())
            .padding(.top, 2)
            // 拒绝 / 接收
            HStack(spacing: 10) {
                Button(action: onReject) {
                    Label("拒绝", systemImage: "xmark")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .tint(.red)
                Button(action: onAccept) {
                    Label("接收", systemImage: "arrow.down")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
            .padding(.top, 6)
            // 存到…
            Button(action: onSaveTo) {
                Label("存到…（选择位置）", systemImage: "folder.badge.plus")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderless)
            .tint(.indigo)
            .padding(.top, 2)
        }
        .padding(22)
        .background(.background, in: RoundedRectangle(cornerRadius: 24))
        .shadow(color: .black.opacity(0.18), radius: 24, y: 10)
        .frame(maxWidth: 330)
    }
}

/// Files App 目录选择器（"存到…"用）
struct FolderPicker: UIViewControllerRepresentable {
    let onPick: (URL) -> Void

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let p = UIDocumentPickerViewController(forOpeningContentTypes: [.folder],
                                               asCopy: false)
        p.delegate = context.coordinator
        p.allowsMultipleSelection = false
        return p
    }
    func updateUIViewController(_ vc: UIDocumentPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onPick: onPick) }
    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let onPick: (URL) -> Void
        init(onPick: @escaping (URL) -> Void) { self.onPick = onPick }
        func documentPicker(_ controller: UIDocumentPickerViewController,
                            didPickDocumentsAt urls: [URL]) {
            if let u = urls.first { onPick(u) }
        }
    }
}

// MARK: - 设备列表

struct DeviceListView: View {
    @EnvironmentObject var model: AppModel
    @State private var showAdd = false
    @State private var manualAddr = ""
    @State private var showScan = false
    @State private var showSettings = false
    @State private var showQr = false

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
            .navigationTitle(model.effectiveName)
            .toolbar {
                ToolbarItemGroup(placement: .navigationBarTrailing) {
                    Button { showQr = true } label: { Image(systemName: "qrcode") }
                    Button { showScan = true } label: { Image(systemName: "qrcode.viewfinder") }
                    Button { showSettings = true } label: { Image(systemName: "gearshape") }
                    Button { showAdd = true } label: { Image(systemName: "plus") }
                }
            }
            .sheet(isPresented: $showScan) {
                ScanToAdd { addr in
                    if !addr.isEmpty { model.addManual(addr) }
                    showScan = false
                }
            }
            .sheet(isPresented: $showSettings) { SettingsSheet() }
            .sheet(isPresented: $showQr) {
                MyQrSheet(ip: Discovery.myLanIp(), port: model.me.port)
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

// MARK: - 扫码添加（AVFoundation）

struct ScanToAdd: View {
    let onResult: (String) -> Void
    @State private var text = ""

    var body: some View {
        NavigationStack {
            ZStack {
                CameraScanner { code in
                    if text.isEmpty {
                        text = code
                        onResult(normalize(code))
                    }
                }
                .ignoresSafeArea()
                VStack {
                    Spacer()
                    Text("对准电脑端二维码（http://IP:端口）")
                        .padding(10)
                        .background(.ultraThinMaterial, in: Capsule())
                    Spacer()
                }
            }
            .navigationTitle("扫码添加")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private func normalize(_ s: String) -> String {
        s.replacingOccurrences(of: "http://", with: "")
            .trimmingCharacters(in: CharacterSet(charactersIn: "/ "))
    }
}

/// 相机二维码扫描（AVCaptureMetadataOutput）
struct CameraScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> UIViewController {
        let vc = ScannerViewController()
        vc.onCode = onCode
        return vc
    }
    func updateUIViewController(_ vc: UIViewController, context: Context) {}

    final class ScannerViewController: UIViewController,
                                        AVCaptureMetadataOutputObjectsDelegate {
        var onCode: ((String) -> Void)?
        private let session = AVCaptureSession()

        override func viewDidLoad() {
            super.viewDidLoad()
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input) else { return }
            session.addInput(input)
            let out = AVCaptureMetadataOutput()
            guard session.canAddOutput(out) else { return }
            session.addOutput(out)
            out.setMetadataObjectsDelegate(self, queue: .main)
            out.metadataObjectTypes = [.qr]
            let prev = AVCaptureVideoPreviewLayer(session: session)
            prev.videoGravity = .resizeAspectFill
            view.layer.addSublayer(prev)
            prev.frame = CGRect(x: 0, y: 0, width: UIScreen.main.bounds.width,
                                height: UIScreen.main.bounds.height)
            DispatchQueue.global(qos: .userInitiated).async { [session] in
                session.startRunning()
            }
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            session.stopRunning()
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput,
                            didOutput metadataObjects: [AVMetadataObject],
                            from connection: AVCaptureConnection) {
            guard let obj = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  obj.type == .qr, let s = obj.stringValue else { return }
            onCode?(s)
        }
    }
}

// MARK: - 本机二维码（CoreImage 生成，无第三方依赖）

struct MyQrSheet: View {
    let ip: String?
    let port: Int
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 14) {
            Text("本机二维码").font(.headline)
            if let ip = ip, port > 0 {
                let s = "http://\(ip):\(port)"
                if let img = qrImage(s) {
                    Image(uiImage: img)
                        .interpolation(.none)
                        .resizable().scaledToFit().frame(width: 210, height: 210)
                        .padding(10).background(.white)
                        .cornerRadius(12)
                }
                Text(s).font(.footnote.monospaced())
                Button("复制地址") { UIPasteboard.general.string = s }
                    .buttonStyle(.bordered)
            } else {
                Text("暂无局域网地址（请连接 Wi-Fi）").foregroundColor(.secondary)
            }
        }
        .padding()
        .presentationDetents([.medium])
    }

    private func qrImage(_ s: String) -> UIImage? {
        let f = CIFilter.qrCodeGenerator()
        f.message = Data(s.utf8)
        f.correctionLevel = "M"
        guard let out = f.outputImage else { return nil }
        let scaled = out.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let ctx = CIContext()
        guard let cg = ctx.createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}

// MARK: - 设置（改名 / 自动接收）

struct SettingsSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @AppStorage("auto_receive") private var autoReceive = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("设备名", text: $name)
                    HStack {
                        Button {
                            name = UIDevice.current.name
                        } label: {
                            Label("使用本机设备名", systemImage: "iphone")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                        Button {
                            name = randomPoeticName()
                        } label: {
                            Label("来个有诗意的名字", systemImage: "sparkles")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                    }
                } header: {
                    Text("设备名")
                } footer: {
                    Text("留空使用系统设备名（当前：\(UIDevice.current.name)）")
                }
                Section("接收") {
                    Toggle("自动接收文件", isOn: $autoReceive)
                }
            }
            .navigationTitle("设置")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("保存") {
                        model.rename(name)
                        dismiss()
                    }
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
            }
        }
        .onAppear { name = model.deviceNameCustom }
    }
}

// MARK: - 会话

struct ChatView: View {
    @EnvironmentObject var model: AppModel
    let peerId: String
    @State private var input = ""
    @State private var showPicker = false
    @State private var showFolderPicker = false
    @State private var confirmClear = false

    private var activeProgress: [MiniHTTPServer.ProgressInfo] {
        model.progress.values.filter { $0.peerId == peerId }
    }

    var body: some View {
        VStack(spacing: 0) {
            statusLine
            messageList
            inputBar
        }
        .navigationTitle(model.disc.peers[peerId]?.info.name ?? "设备")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { clearToolbar }
        .fileImporter(isPresented: $showPicker, allowedContentTypes: [.data],
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { model.sendPicked(urls) }
        }
        .fileImporter(isPresented: $showFolderPicker, allowedContentTypes: [.folder],
                      allowsMultipleSelection: false) { result in
            if case .success(let urls) = result, let u = urls.first {
                model.sendFolder(u)
            }
        }
    }

    @ViewBuilder private var statusLine: some View {
        if let s = model.sendStatus {
            Text(s).font(.caption).frame(maxWidth: .infinity)
                .padding(.vertical, 4)
                .background(Color(.secondarySystemBackground).opacity(0.5))
        }
    }

    private var messageList: some View {
        ScrollView {
            LazyVStack(spacing: 6) {
                ForEach(model.chats[peerId] ?? []) { e in
                    self.chatRow(e)
                }
                ForEach(activeProgress, id: \.label) { p in
                    self.progressRow(p)
                }
            }
            .padding()
        }
    }

    private func chatRow(_ e: ChatEntry) -> some View {
        Bubble(entry: e, speed: tokenFor(e).flatMap { model.speeds[$0] })
            .contextMenu { rowMenu(e) }
    }

    @ViewBuilder private func rowMenu(_ e: ChatEntry) -> some View {
        Button(role: .destructive) {
            model.deleteEntry(e.id, in: peerId)
        } label: {
            Label("删除", systemImage: "trash")
        }
        if e.isFile, let files = e.files {
            ForEach(Array(files.prefix(8))) { f in
                Button {
                    if let url = f.url { UIApplication.shared.open(url) }
                } label: {
                    Label("打开 \(f.name)", systemImage: "doc")
                }
            }
        }
    }

    private func progressRow(_ p: MiniHTTPServer.ProgressInfo) -> some View {
        let key = model.progress.first { $0.value.label == p.label }?.key
        let speed = key.flatMap { model.speeds[$0] }
        let entry = ChatEntry.progress(label: p.label, idx: p.fileIdx,
                                       count: p.fileCount,
                                       transferred: p.transferred, total: p.total)
        return Bubble(entry: entry, speed: speed)
    }

    private var inputBar: some View {
        HStack {
            Button { showPicker = true } label: { Image(systemName: "paperclip") }
                .buttonStyle(.bordered)
            Button { showFolderPicker = true } label: { Image(systemName: "folder") }
                .buttonStyle(.bordered)
            Button { model.sendClipboard() } label: { Image(systemName: "doc.on.clipboard") }
                .buttonStyle(.bordered)
            TextField("输入消息", text: $input, axis: .vertical)
                .textFieldStyle(.roundedBorder).lineLimit(1...4)
                .onSubmit(send)
            Button("发送", action: send).buttonStyle(.borderedProminent)
        }
        .padding(8)
    }

    private var clearToolbar: some ToolbarContent {
        ToolbarItem(placement: .navigationBarTrailing) {
            Button(role: .destructive) {
                if confirmClear {
                    model.clearChat(peerId); confirmClear = false
                } else {
                    confirmClear = true
                    DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                        confirmClear = false
                    }
                }
            } label: {
                Image(systemName: confirmClear ? "checkmark.circle" : "trash.slash")
            }
        }
    }

    /// 进度卡的速度 key：找 label 对应的 token（发送键以 send- 开头）
    private func tokenFor(_ e: ChatEntry) -> String? {
        guard e.isProgress, let label = e.progressLabel else { return nil }
        return model.progress.first { $0.value.label == label }?.key
    }

    private func send() {
        let t = input.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty else { return }
        input = ""
        model.sendText(t)
    }
}

// MARK: - 气泡

struct Bubble: View {
    let entry: ChatEntry
    var speed: Double?

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
        switch entry.kind {
        case .text(let t):
            // 部分复制：原生文本选择（长按出选择手柄，系统菜单含拷贝）
            Text(t).foregroundColor(entry.outgoing ? .white : .primary)
                .textSelection(.enabled)
        case .fileCard(let title, let size, let location, let files):
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
                HStack(spacing: 6) {
                    Text(transferred >= 0
                         ? "\(total > 0 ? transferred * 100 / total : 0)% · " +
                           "\(fmtSize(transferred)) / \(fmtSize(total))"
                         : "")
                        .font(.caption2).foregroundColor(.secondary)
                    if let sp = speed, sp > 0 {
                        Text(fmtSpeed(sp)).font(.caption2).foregroundColor(.secondary)
                    }
                }
                ProgressView(value: total > 0 ? Double(transferred) / Double(total) : 0)
            }
        }
    }
}
