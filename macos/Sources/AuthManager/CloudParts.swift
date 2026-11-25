import AppKit
import SwiftUI

// Pieces for accounts the authentication centre renews: the small cloud on a card, the ring while an account
// moves to the centre or back, the box a failed move leaves, and the confirmations for each move.

// MARK: 云形小标

// A cloud in a fixed outline, drawn solid and centred in the frame it gets.
struct CloudShape: Shape {
    private static let outline = SVGPath.parse("M4.6 12.4h6.8a2.8 2.8 0 0 0 .4-5.57A4.1 4.1 0 0 0 3.9 5.7a2.6 2.6 0 0 0 .7 6.7Z")

    func path(in rect: CGRect) -> Path {
        let box = Self.outline.boundingRect
        let scale = min(rect.width / box.width, rect.height / box.height)
        let x = rect.midX - box.midX * scale, y = rect.midY - box.midY * scale
        return Self.outline.applying(CGAffineTransform(translationX: x, y: y).scaledBy(x: scale, y: scale))
    }
}

// Next to an account's name: a grey cloud when the centre renews it (amber when it could not), a turning ring
// while it moves to the centre or back, nothing for an account this Mac renews. Hovering says what it means.
struct CloudBadge: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    var size: CGFloat = 12

    var body: some View {
        if account.transferRunning {
            TurningRing(size: size + 1)
                .help(account.health.title)
                .accessibilityLabel(account.health.title)
        } else if account.cloudRenewed {
            CloudShape().fill(account.cloudTrouble != nil ? Palette.attention : Palette.ink3)
                .frame(width: size, height: size)
                .padding(2)
                .contentShape(Rectangle())
                .help(Self.help(account, service.directory))
                .accessibilityElement()
                .accessibilityLabel(account.cloudTrouble != nil ? "云端续期出错" : "由认证中心续期")
        }
    }

    // Two lines: who renews it and on how many Macs it can be used; what switching to it asks of AuthBaton.
    static func help(_ account: ManagedAccount, _ directory: AccountDirectory?) -> String {
        if let trouble = account.cloudTrouble { return "云端续期出错\n" + trouble }
        let macs = directory?.cloud?.devices.count ?? 0
        var lines = ["由认证中心续期" + (macs > 0 ? " · \(macs) 台 Mac 可用" : "")]
        let others = (account.cloud?.usedBy ?? []).filter { $0.this != true }.map(\.label)
        if account.native {
            if !others.isEmpty { lines.append("\(others.joined(separator: "、")) 也在用此账号") }
            lines.append("请保持 AuthBaton 运行，否则约 \(account.cloudLapse)后掉登录。")
        } else {
            lines.append("换上后需保持 AuthBaton 运行，否则约 \(account.cloudLapse)后掉登录。")
        }
        return lines.joined(separator: "\n")
    }
}

// A thin brand ring turning in place: the work it stands for is under way.
struct TurningRing: View {
    var size: CGFloat = 13
    @State private var turning = false

    var body: some View {
        ZStack {
            Circle().stroke(Palette.soft, lineWidth: 1.6)
            Circle().trim(from: 0, to: 0.3).stroke(Palette.brand, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
                .rotationEffect(.degrees(turning ? 360 : 0))
                .animation(.linear(duration: 0.9).repeatForever(autoreverses: false), value: turning)
        }
        .frame(width: size - 2, height: size - 2)
        .frame(width: size, height: size)
        .onAppear { turning = true }
    }
}

// MARK: 转换没成

// A move that stopped: why, that the account stays where it was, and the two ways on. 放弃 leaves it with whoever
// renewed it before; 重试 repeats the same operation.
struct TransferBox: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount

    var body: some View {
        if let transfer = account.transferFailed {
            let back = transfer.direction == "to_local"
            HStack(alignment: .center, spacing: 8) {
                Icon(.alert, size: 13, color: Palette.attention)
                Text((back ? "转回本机失败：" : "转到云端失败：") + Self.clause(transfer.error ?? "")
                     + (back ? "，仍由云端续期。" : "，当前仍由本机续期。"))
                    .font(.system(size: 11)).foregroundStyle(Palette.attentionText)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                Button("放弃") { Task { await service.cloudAction(back ? "cancel_export" : "cancel_import", id: account.id, confirm: true) } }
                    .buttonStyle(.pill(.onAttention, height: 22))
                    .overlay(Capsule().strokeBorder(Palette.attentionDash.opacity(0.6), lineWidth: 1))
                    .help(back ? "不转了，继续由云端续期" : "不转了，继续由这台 Mac 续期")
                Button("重试") { Task { await service.cloudAction(back ? "export" : "resume_import", id: account.id, confirm: true) } }
                    .buttonStyle(.pill(.attention, height: 22))
            }
            .disabled(service.accountBusy)
            .padding(.horizontal, 10).padding(.vertical, 8)
            .background(Palette.attentionFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
    }

    // The service's sentence, to run on into ours.
    private static func clause(_ text: String) -> String {
        var text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        while let last = text.last, "。.，,".contains(last) { text.removeLast() }
        return text
    }
}

// MARK: 流向条

// One end of a flow: this Mac or the authentication centre.
struct FlowEnd {
    let title: String
    let caption: String
    var pill: String? = nil
    var centre = false
    // The centre not connected yet: a dashed tile.
    var dashed = false

    // 本机, under it this Mac's name; 在用 when the account is in use here.
    static func mac(_ name: String, inUse: Bool = false) -> FlowEnd {
        FlowEnd(title: "本机", caption: name, pill: inUse ? "在用" : nil)
    }
    static let centre = FlowEnd(title: "认证中心", caption: "云端续期", centre: true)
}

// Where the renewal goes: an end on each side, a line between, and what the line means when it is not a move.
struct CloudFlow: View {
    let from: FlowEnd
    let to: FlowEnd
    // Over the line: 已连接 · 实时同步, 未配对. Without it the line is an arrow.
    var caption: String? = nil
    var linked = true

    var body: some View {
        HStack(spacing: 12) {
            side(from)
            VStack(spacing: 4) {
                RelayLine(style: linked ? .open : .idle, thickness: 1.5, arrow: caption == nil)
                if let caption {
                    Text(caption).font(.system(size: 10)).foregroundStyle(linked ? Palette.deep : Palette.ink3).lineLimit(1)
                }
            }
            .padding(.top, caption == nil ? 0 : 12)
            .frame(minWidth: 40, maxWidth: .infinity)
            side(to)
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .background(Palette.field, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func side(_ end: FlowEnd) -> some View {
        let shape = RoundedRectangle(cornerRadius: 9, style: .continuous)
        return HStack(spacing: 9) {
            Group {
                if end.centre {
                    CloudShape().fill(end.dashed ? Palette.ink3 : Palette.brand).frame(width: 16, height: 16)
                } else {
                    Icon(.mac, size: 17, color: Palette.ink2)
                }
            }
            .frame(width: 34, height: 34)
            .background(end.centre && !end.dashed ? Palette.wash : end.dashed ? Color.clear : Palette.card, in: shape)
            .overlay {
                if end.dashed { shape.strokeBorder(Palette.dashStrong, style: StrokeStyle(lineWidth: 1, dash: [3, 2.5])) }
                else if !end.centre { shape.strokeBorder(Palette.ring, lineWidth: 1) }
            }
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 5) {
                    Text(end.title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                    if let pill = end.pill { Pill(pill, style: .brand, small: true) }
                }
                Text(end.caption).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
            }
        }
        .fixedSize()
    }
}

// An amber box in a confirmation: what the person has to do, or what they will lose.
struct AmberNote<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Icon(.alert, size: 13, color: Palette.attention).padding(.top, 1)
            VStack(alignment: .leading, spacing: 6) { content }
                .font(.system(size: 11)).foregroundStyle(Palette.attentionText)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.attentionDash.opacity(0.45), lineWidth: 1))
    }
}

// A quiet line with its glyph, as under the flow in the confirmations.
struct NoteLine: View {
    let glyph: Glyph
    let text: String
    var color: Color = Palette.ink3

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Icon(glyph, size: 13, color: color).padding(.top, 1)
            Text(text).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: 转到云端

struct ToCloudSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void
    @State private var working = false

    // The client that has to stop first when the account is in use: the service will not take it over while it runs.
    private var client: (name: String, short: String) {
        account.provider == "codex" ? ("Codex CLI", "CLI") : ("Claude Code", "Claude Code")
    }

    var body: some View {
        let directory = service.directory
        let devices = directory?.cloud?.devices ?? []
        SheetScaffold(title: "将「\(account.displayName)」转到云端续期？", lead: "交由认证中心保管并自动续期，无需重新登录。") {
            CloudFlow(from: .mac(directory?.thisMac ?? "这台 Mac", inUse: account.native), to: .centre)
            if !devices.isEmpty {
                VStack(alignment: .leading, spacing: 7) {
                    NoteLine(glyph: .desktop, text: "以下已配对 Mac 均可见并可直接换上：")
                    DeviceChips(devices: devices).padding(.leading, 21)
                }
            }
            if account.native {
                AmberNote {
                    Text("请先退出正在运行的 \(client.name)。转到云端后，本机客户端将改为短期凭据，需保持 AuthBaton 运行（否则约 \(account.cloudLapse)后掉登录）。")
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                NoteLine(glyph: .clock, text: "换上后客户端仅拿短期凭据，需保持 AuthBaton 运行（否则约 \(account.cloudLapse)后掉登录）。")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(working ? "正在转到云端…" : account.native ? "已退出 \(client.short)，转到云端" : "转到云端续期") {
                working = true
                Task {
                    await submit(service, "accounts.cloud", LoginParams(id: account.id, confirm: true, cloud_action: "import"), close: close)
                    working = false
                }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
            .disabled(working || service.accountBusy || directory?.cloud?.busy == true)
        }
    }
}

// The paired Macs as chips, this one marked.
struct DeviceChips: View {
    let devices: [CloudDevice]

    var body: some View {
        HStack(spacing: 6) {
            ForEach(devices) { device in
                HStack(spacing: 4) {
                    Text(device.label).foregroundStyle(Palette.ink)
                    if device.this == true { Text("这台").foregroundStyle(Palette.ink3) }
                }
                .font(.system(size: 11, weight: .medium)).lineLimit(1)
                .padding(.horizontal, 8).frame(height: 22)
                .background(Palette.chip, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            }
        }
    }
}

// Not paired yet: the account cannot move until this Mac is connected to the person's own centre.
struct PairFirstSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void

    var body: some View {
        SheetScaffold(title: "先配对个人云端", lead: "将「\(account.displayName)」交给云端续期前，需要先连接到你自己的认证中心。") {
            CloudFlow(from: FlowEnd(title: service.directory?.thisMac ?? "这台 Mac", caption: "这台 Mac"),
                      to: FlowEnd(title: "认证中心", caption: "未连接", centre: true, dashed: true), caption: "未配对", linked: false)
            NoteLine(glyph: .relay, text: "配对完成后，可直接把已有账号转到云端，无需重新登录。", color: Palette.solid)
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("前往个人云端") {
                close()
                AppNavigation.shared.show(.cloud)
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
        }
    }
}

// MARK: 转回本机

struct ToLocalSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void
    @State private var working = false

    var body: some View {
        let directory = service.directory
        let mac = directory?.thisMac ?? "这台 Mac"
        let used = account.cloud?.usedBy ?? []
        let others = used.filter { $0.this != true }
        let here = used.first { $0.this == true }
        SheetScaffold(title: "将「\(account.displayName)」转回本机续期？", lead: "凭据迁回这台 Mac，认证中心不再保管，无需重新登录。") {
            CloudFlow(from: .centre, to: .mac(mac))
            if others.isEmpty {
                NoteLine(glyph: .check, text: here != nil ? "近期仅 \(mac)（这台）在使用，不影响其他已配对 Mac。" : "近期没有别的 Mac 在用，不影响其他已配对 Mac。",
                         color: Palette.solid)
            } else {
                AmberNote {
                    Text("转回后，\(others.map(\.label).joined(separator: "、")) 将失去此账号").font(.system(size: 12, weight: .semibold))
                    ForEach(Array(others.enumerated()), id: \.offset) { _, use in
                        row(use.label + " · " + When.text(use.at), lost: true)
                    }
                    if let here { row("\(mac)（这台）· " + When.text(here.at), lost: false) }
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(working ? "正在转回本机…" : "转回本机续期") {
                working = true
                Task {
                    await submit(service, "accounts.cloud", LoginParams(id: account.id, confirm: true, cloud_action: "export"), close: close)
                    working = false
                }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
            .disabled(working || service.accountBusy || directory?.cloud?.busy == true)
        }
    }

    private func row(_ text: String, lost: Bool) -> some View {
        HStack(spacing: 6) {
            Icon(.mac, size: 12, color: lost ? Palette.attention : Palette.ink3)
            Text(text).foregroundStyle(lost ? Palette.attentionText : Palette.ink2).lineLimit(1)
            Spacer(minLength: 6)
            Icon(lost ? .close : .check, size: 11, color: lost ? Palette.attention : Palette.solid, weight: 2)
        }
    }
}

// MARK: 从认证中心移除

struct CloudRemoveSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void

    var body: some View {
        SheetScaffold(title: "从认证中心移除「\(account.displayName)」？", lead: "认证中心删掉这个账号的登录，所有已配对的 Mac 都不能再用它。") {
            VStack(alignment: .leading, spacing: 9) {
                CheckItem(kind: .you, text: "这台 Mac 上的这张卡一起移除；以后再用要重新登录。")
                CheckItem(kind: .info, text: "已经发出的短期凭据不会被撤销，到期后失效。")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("从认证中心移除", role: .destructive) {
                Task { await submit(service, "accounts.cloud", LoginParams(id: account.id, confirm: true, cloud_action: "remove"), close: close) }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy)
        }
    }
}

// MARK: 断开配对

struct CloudLeaveSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void

    var body: some View {
        let inUse = service.directory?.cloudInUse ?? []
        SheetScaffold(title: "断开与认证中心的配对？", lead: "这台 Mac 不再领取云端续期账号的短期凭据。") {
            VStack(alignment: .leading, spacing: 9) {
                CheckItem(kind: .you, text: "别的 Mac 转上来的账号从这台移除；从这台转上去的要重新登录。")
                CheckItem(kind: .system, text: "认证中心里的账号不受影响，别的 Mac 照常用。")
            }
            if !inUse.isEmpty {
                AmberNote {
                    Text("在用的 \(inUse.map(\.displayName).joined(separator: "、")) 由云端续期，先换上本机续期的账号再断开。")
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("断开配对", role: .destructive) {
                Task { await submit(service, "accounts.cloud", LoginParams(confirm: true, cloud_action: "leave"), close: close) }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy || !inUse.isEmpty)
        }
    }
}
