import SwiftUI

// Safe network switching (v0.22). Pick the next network; AuthBaton holds Claude Code's new
// requests, waits for every reply in flight to end (5 minutes at most) and only then switches.

enum NetworkSheet: Identifiable {
    case pick(NetworkTarget?)
    case progress

    var id: String {
        switch self {
        case .pick(let target): "pick:\(target?.id ?? "")"
        case .progress: "progress"
        }
    }
}

struct NetworkPage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @State private var sheet: NetworkSheet?
    @State private var hovered: String?
    @State private var showAll = false

    private var status: NetworkStatus? { service.directory?.network }

    var body: some View {
        PageScaffold(title: "网络", subtitle: Text(status?.current.title.prefixed("现在连着 ") ?? "正在读取…"), glyph: .wifi) {
            if status?.switching != true {
                Button { sheet = .pick(nil) } label: {
                    HStack(spacing: 5) { Icon(.relay, size: 12, color: .white); Text("切换网络") }
                }
                .buttonStyle(.pill(.primary, height: 27))
                .disabled(status?.available != true)
            }
        } content: {
            if let status {
                if !status.available {
                    Banner(text: "这台 Mac 没有 Wi-Fi，切换不了网络。")
                }
                if let op = status.operation, !op.running, op.step == "wrong_password", op.kind == "wifi" {
                    Banner(glyph: .key, title: "\(op.target) 的密码不对", text: op.message ?? "已连回原来的网络，这个密码没有记住。") {
                        Button("重新输入") { sheet = .pick(.wifi(op.target)) }.buttonStyle(.pill(.primary, height: 22))
                    }
                }
                timing(status)
                targets(status)
                if !status.history.isEmpty { history(status) }
            }
        }
        .task {
            while !Task.isCancelled {
                await service.watchNetwork()
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .onAppear(perform: takeRequest)
        .onChange(of: nav.networkSheet) { _, _ in takeRequest() }
        .sheet(item: $sheet) { item in
            switch item {
            case .pick(let target):
                NetworkSwitchSheet(initial: target) { sheet = .progress } close: { sheet = nil }
                    .environmentObject(service)
            case .progress:
                NetworkProgressSheet { sheet = nil } retry: { target in sheet = .pick(target) }
                    .environmentObject(service)
            }
        }
    }

    // The menu bar's 切网 opens this page with the picker, or the progress of a running switch.
    private func takeRequest() {
        guard nav.networkSheet else { return }
        nav.networkSheet = false
        sheet = status?.switching == true ? .progress : .pick(nil)
    }

    // MARK: 切网时机

    private func timing(_ status: NetworkStatus) -> some View {
        let op = status.operation?.running == true ? status.operation : nil
        return VStack(alignment: .leading, spacing: 7) {
            Text("切网时机").font(.system(size: 13, weight: .semibold))
            HStack(alignment: .center, spacing: 26) {
                TimingRing(state: op != nil ? .waiting(Double(op?.remaining ?? 0) / 300) : status.safe ? .safe : .busy)
                    .frame(width: 168, height: 168)
                VStack(alignment: .leading, spacing: 5) {
                    if let op {
                        figure(countdown(op.remaining ?? 0), unit: "后不切了")
                        Text(op.step == "waiting" ? "等回复都传完，就切到 \(op.target)。" : "正在切到 \(op.target)。")
                            .font(.system(size: 12)).foregroundStyle(Palette.ink2)
                        HStack(spacing: 7) {
                            Button("取消切换") { Task { await service.network("cancel") } }
                                .buttonStyle(.pill(.secondary, height: 27))
                                .disabled(!["waiting", "waking"].contains(op.step))
                            Button("看进度") { sheet = .progress }.buttonStyle(.pill(.quiet, height: 27))
                        }
                        .padding(.top, 7)
                    } else if status.safe {
                        Text("可以切").font(.system(size: 28, weight: .bold)).kerning(-0.3)
                        Text("没有回复在传输，现在切会马上切。").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    } else {
                        figure("\(status.live.count)", unit: "个回复在传输")
                        Text("现在切会把它们截断。点了切换，Claude Code 先暂停发新请求，都传完就切，最多等 5 分钟。")
                            .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !status.sources.isEmpty {
                VStack(spacing: 0) {
                    ForEach(status.sources) { source in sourceRow(source, waiting: op != nil) }
                }
                .padding(.top, 7)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
                .padding(.top, 7)
            }
        }
        .padding(.horizontal, 22).padding(.top, 20).padding(.bottom, 19)
        .loopCard(radius: 19, raised: true)
    }

    private func figure(_ value: String, unit: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(value).font(.system(size: 44, weight: .semibold, design: .rounded)).monospacedDigit().kerning(-0.9)
            Text(unit).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.ink3)
        }
    }

    private func countdown(_ seconds: Int) -> String { String(format: "%d:%02d", seconds / 60, seconds % 60) }

    private func sourceRow(_ source: NetworkStatus.Source, waiting: Bool) -> some View {
        let (label, help): (String, String) = switch source.state {
        case "live" where waiting && !source.gated: ("在传输 · 只能等", "它拦不住新请求，只能等它传完")
        case "live": ("在传输", "有回复正在传输")
        case "held": ("已暂停新请求", "这次回复传完了，新请求等切完网再发")
        default: ("空闲", "空闲")
        }
        return HStack(spacing: 9) {
            Icon(sourceGlyph(source.name), size: 14, color: sourceTint(source.name))
            Text(source.name).font(.system(size: 12)).lineLimit(1)
            Spacer(minLength: 7)
            HStack(spacing: 5) {
                Circle().fill(source.state == "live" ? Palette.brand : source.state == "held" ? Palette.solid : .clear)
                    .overlay { if source.state == "idle" { Circle().strokeBorder(Palette.dashStrong, lineWidth: 1.2) } }
                    .padding(source.state == "held" ? 0 : 0)
                    .frame(width: 6, height: 6)
                    .background { if source.state == "held" { Circle().fill(Palette.soft).frame(width: 10, height: 10) } }
                Text(label).font(.system(size: 11)).foregroundStyle(source.state == "idle" ? Palette.ink3 : Palette.ink)
            }
            .help(help)
        }
        .frame(height: 32)
        .accessibilityElement(children: .combine)
    }

    private func sourceGlyph(_ name: String) -> Glyph {
        if name.hasPrefix("Claude Code") { return .claude }
        if name.hasPrefix("Codex") { return .codex }
        if name.hasPrefix("Claude") { return .desktop }
        return .window
    }

    private func sourceTint(_ name: String) -> Color {
        if name.hasPrefix("Codex") { return Palette.codex }
        if name.hasPrefix("Claude") { return Palette.claude }
        return Palette.ink2
    }

    // MARK: 可以切到

    private func targets(_ status: NetworkStatus) -> some View {
        let networks = showAll ? status.networks : Array(status.networks.prefix(5))
        return VStack(alignment: .leading, spacing: 0) {
            Text("可以切到").font(.system(size: 13, weight: .semibold))
            if !status.phones.isEmpty {
                groupLabel("iPhone 热点").padding(.top, 12)
                ForEach(status.phones) { phone in phoneRow(phone, disabled: status.switching) }
            }
            groupLabel("Wi-Fi").padding(.top, status.phones.isEmpty ? 12 : 10)
            if networks.isEmpty {
                Text("没读到这台 Mac 记住的 Wi-Fi。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(9)
            }
            ForEach(networks) { network in wifiRow(network, disabled: status.switching) }
            if status.networks.count > 5 {
                Button(showAll ? "收起" : "显示全部 \(status.networks.count) 个") { showAll.toggle() }
                    .buttonStyle(.pill(.link, height: 26)).padding(.top, 5)
            }
        }
        .padding(.horizontal, 22).padding(.top, 20).padding(.bottom, 19)
        .loopCard(radius: 19, raised: true)
    }

    private func groupLabel(_ text: String) -> some View {
        Text(text).font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.ink3)
            .padding(.horizontal, 9).padding(.bottom, 3)
    }

    private func phoneRow(_ phone: NetworkStatus.Phone, disabled: Bool) -> some View {
        HStack(spacing: 10) {
            Icon(.hotspot, size: 14, color: Palette.solid)
                .frame(width: 24, height: 24)
                .background(Palette.soft, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            Text(phone.name).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            if let signal = phone.signal { SignalBars(level: signal) }
            if let network = phone.network { Text(network).font(.system(size: 11)).foregroundStyle(Palette.ink3) }
            if let battery = phone.battery { BatteryGauge(percent: battery) }
            Spacer(minLength: 7)
            Button("切到这里") { sheet = .pick(.phone(phone.name)) }
                .buttonStyle(.pill(.secondary, height: 24)).disabled(disabled)
                .help("AuthBaton 会远程打开热点，不用碰手机，也不用密码")
        }
        .padding(.horizontal, 9).frame(height: 40)
    }

    private func wifiRow(_ network: NetworkStatus.Network, disabled: Bool) -> some View {
        let hover = hovered == network.ssid && !network.current
        return HStack(spacing: 10) {
            Icon(.wifi, size: 14, color: Palette.ink2).frame(width: 24)
            Text(network.ssid).font(.system(size: 12)).lineLimit(1)
            if network.remembered {
                Icon(.key, size: 12, color: Palette.ink3).help("记住了密码").accessibilityLabel("记住了密码")
            }
            Spacer(minLength: 7)
            if network.current {
                Pill("现在", small: true)
            } else if hover {
                if network.remembered {
                    Button("忘记密码") { Task { await service.network("forget", ssid: network.ssid) } }
                        .buttonStyle(.pill(.quiet, height: 24))
                }
                Button("切到这里") { sheet = .pick(.wifi(network.ssid)) }
                    .buttonStyle(.pill(.secondary, height: 24)).disabled(disabled)
            }
        }
        .padding(.horizontal, 9).frame(height: 36)
        .background(hover ? Palette.ground : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
        .onHover { inside in hovered = inside ? network.ssid : (hovered == network.ssid ? nil : hovered) }
    }

    // MARK: 最近切换

    private func history(_ status: NetworkStatus) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("最近切换").font(.system(size: 13, weight: .semibold)).padding(.bottom, 9)
            ForEach(status.history.prefix(5)) { record in
                HStack(spacing: 10) {
                    ResultMark(result: record.result)
                    Text(When.text(record.at)).font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit()
                        .frame(width: 76, alignment: .leading)
                    Text("\(record.from) → \(record.to)").font(.system(size: 12)).lineLimit(1)
                        .foregroundStyle(record.result == "done" ? Palette.ink : Palette.ink2)
                    Spacer(minLength: 7)
                    Text(outcome(record)).font(.system(size: 11)).lineLimit(1)
                        .foregroundStyle(["wrong_password", "failed", "unreachable", "not_found"].contains(record.result) ? Palette.attentionText : Palette.ink3)
                }
                .frame(minHeight: 34)
            }
        }
        .padding(.horizontal, 22).padding(.top, 20).padding(.bottom, 19)
        .loopCard(radius: 19)
    }

    private func outcome(_ record: NetworkStatus.Record) -> String {
        if record.result == "done" { return (record.waited ?? 0) > 0 ? "等了 \(record.waited ?? 0) 秒" : "没等" }
        return record.message ?? "没切"
    }
}

private extension String {
    func prefixed(_ prefix: String) -> String { prefix + self }
}

// The 切网时机 signature: a calm ring when nothing is in flight, brand arcs while replies stream,
// and the 5-minute countdown with a pause mark while Claude Code holds its next request.
struct TimingRing: View {
    enum State: Equatable { case safe, busy, waiting(Double) }
    let state: State

    var body: some View {
        ZStack {
            Circle().stroke(Palette.soft, lineWidth: 9).padding(17.5)
            switch state {
            case .safe:
                Path { p in
                    p.move(to: CGPoint(x: 66, y: 86)); p.addLine(to: CGPoint(x: 78, y: 98)); p.addLine(to: CGPoint(x: 103, y: 72))
                }
                .stroke(Palette.solid, style: StrokeStyle(lineWidth: 6, lineCap: .round, lineJoin: .round))
            case .busy:
                ForEach([0.0, 0.25], id: \.self) { start in
                    Circle().trim(from: start, to: start + 0.164)
                        .stroke(Palette.brand, style: StrokeStyle(lineWidth: 9, lineCap: .round))
                        .rotationEffect(.degrees(-70)).padding(17.5)
                }
                HStack(spacing: 6) {
                    Circle().fill(Palette.soft).frame(width: 8, height: 8)
                    Circle().fill(Palette.brand).frame(width: 8, height: 8)
                    Circle().fill(Palette.solid).frame(width: 8, height: 8)
                }
            case .waiting(let left):
                Circle().trim(from: 0, to: max(0.02, min(1, left)))
                    .stroke(Palette.brand, style: StrokeStyle(lineWidth: 9, lineCap: .round))
                    .rotationEffect(.degrees(-90)).padding(17.5)
                HStack(spacing: 10) {
                    RoundedRectangle(cornerRadius: 3).fill(Palette.solid).frame(width: 9, height: 36)
                    RoundedRectangle(cornerRadius: 3).fill(Palette.solid).frame(width: 9, height: 36)
                }
            }
        }
        .frame(width: 168, height: 168)
        .accessibilityElement()
        .accessibilityLabel(state == .safe ? "可以切网" : state == .busy ? "有回复在传输" : "等着切网")
    }
}

struct SignalBars: View {
    let level: Int
    var body: some View {
        HStack(alignment: .bottom, spacing: 1.3) {
            ForEach(0..<4) { i in
                RoundedRectangle(cornerRadius: 0.8).fill(i < level ? Palette.ink2 : Palette.dash)
                    .frame(width: 3, height: 4 + CGFloat(i) * 2.33)
            }
        }
        .frame(height: 11, alignment: .bottom)
        .accessibilityLabel("信号 \(level) 格")
    }
}

struct BatteryGauge: View {
    let percent: Int
    var body: some View {
        HStack(spacing: 1) {
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 2.5).strokeBorder(Palette.disabled, lineWidth: 1).frame(width: 19, height: 9)
                RoundedRectangle(cornerRadius: 1).fill(Palette.ink2)
                    .frame(width: 16 * CGFloat(max(0, min(100, percent))) / 100, height: 6).padding(.leading, 1.5)
            }
            RoundedRectangle(cornerRadius: 1).fill(Palette.disabled).frame(width: 1.5, height: 3.5)
        }
        .accessibilityLabel("电量 \(percent)%")
        .help("电量 \(percent)%")
    }
}

struct ResultMark: View {
    let result: String
    var body: some View {
        ZStack {
            Circle().fill(result == "done" ? Palette.soft : bad ? Palette.attentionFill : Palette.chip)
            if result == "done" {
                Icon(.check, size: 10, color: Palette.solid, weight: 2.4)
            } else if bad {
                Text("!").font(.system(size: 10, weight: .bold)).foregroundStyle(Palette.attention)
            } else {
                Capsule().fill(Palette.ink3).frame(width: 6, height: 1.8)
            }
        }
        .frame(width: 16, height: 16)
    }
    private var bad: Bool { ["wrong_password", "failed", "unreachable", "not_found"].contains(result) }
}

// MARK: 切换网络

struct NetworkSwitchSheet: View {
    @EnvironmentObject private var service: ServiceController
    let initial: NetworkTarget?
    let started: () -> Void
    let close: () -> Void
    @State private var pick: NetworkTarget?
    @State private var password = ""
    @State private var sending = false

    private var status: NetworkStatus? { service.directory?.network }

    private var choices: [NetworkTarget] {
        guard let status else { return [] }
        return status.phones.map { .phone($0.name) } + status.networks.filter { !$0.current }.map { .wifi($0.ssid) }
    }

    private func remembered(_ ssid: String) -> Bool { status?.networks.first { $0.ssid == ssid }?.remembered == true }

    private var needsPassword: Bool {
        if case .wifi(let ssid) = pick { return !remembered(ssid) }
        return false
    }

    var body: some View {
        SheetScaffold(title: "切换网络", lead: "等回复都传完再切。Claude Code 会先暂停发新请求；5 分钟内等不到就不切。") {
            VStack(alignment: .leading, spacing: 2) {
                Text("切到").font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.ink3).padding(.bottom, 3)
                ScrollView {
                    VStack(spacing: 2) { ForEach(choices) { choice in row(choice) } }
                }
                .frame(maxHeight: 200)
                .fixedSize(horizontal: false, vertical: choices.count <= 5)
            }
            if needsPassword, case .wifi(let ssid) = pick {
                VStack(alignment: .leading, spacing: 5) {
                    Text("\(ssid) 的密码").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    LoopField(placeholder: "Wi-Fi 密码", text: $password, secure: true)
                    Text("连上以后记住，密码改了才会再问。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            if case .phone = pick {
                HStack(spacing: 7) {
                    Icon(.phone, size: 12, color: Palette.ink3)
                    Text("会远程打开 iPhone 的热点，不用碰手机，也不用密码。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            if let error = service.accountError, !sending {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText)
            }
        } actions: {
            Button("取消") { close() }.buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("切换") { Task { await start() } }
                .buttonStyle(.pill(.primary, height: 27))
                .keyboardShortcut(.defaultAction)
                .disabled(pick == nil || sending || service.accountBusy || (needsPassword && password.count < 8))
        }
        .onAppear {
            service.clearAccountError()
            pick = initial ?? choices.first
        }
    }

    private func row(_ choice: NetworkTarget) -> some View {
        let on = pick == choice
        return Button { pick = choice; password = "" } label: {
            HStack(spacing: 10) {
                ZStack {
                    Circle().strokeBorder(on ? Palette.solid : Palette.dashStrong, lineWidth: on ? 4.5 : 1.2)
                }
                .frame(width: 14, height: 14)
                switch choice {
                case .phone(let name):
                    Icon(.hotspot, size: 14, color: Palette.solid)
                    Text(name).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    Spacer(minLength: 5)
                    if let phone = status?.phones.first(where: { $0.name == name }) {
                        Text([phone.network, phone.battery.map { "\($0)%" }].compactMap { $0 }.joined(separator: " · "))
                            .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                case .wifi(let ssid):
                    Icon(.wifi, size: 14, color: Palette.ink2)
                    Text(ssid).font(.system(size: 12)).lineLimit(1)
                    if remembered(ssid) { Icon(.key, size: 12, color: Palette.ink3).help("记住了密码") }
                    Spacer(minLength: 5)
                }
            }
            .padding(.horizontal, 10).frame(height: 38)
            .background(on ? Palette.wash : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay { if on { RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.soft, lineWidth: 1) } }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func start() async {
        guard let pick else { return }
        sending = true
        defer { sending = false }
        switch pick {
        case .phone(let name): await service.network("switch", phone: name)
        case .wifi(let ssid): await service.network("switch", ssid: ssid, password: needsPassword ? password : nil)
        }
        password = ""
        if service.accountError == nil { started() }
    }
}

// MARK: 切换进行中

struct NetworkProgressSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    let retry: (NetworkTarget) -> Void

    private var op: NetworkStatus.Operation? { service.directory?.network?.operation }

    var body: some View {
        let op = op
        let step = op?.step ?? "waiting"
        let phone = op?.kind == "phone"
        let target = op?.target ?? ""
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 5) {
                    Text(title(step, target)).font(.system(size: 14, weight: .bold))
                    Text(lead(op)).font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                if step == "waiting", op?.running == true {
                    ZStack {
                        Circle().stroke(Palette.soft, lineWidth: 4)
                        Circle().trim(from: 0, to: Double(op?.remaining ?? 0) / 300)
                            .stroke(Palette.brand, style: StrokeStyle(lineWidth: 4, lineCap: .round)).rotationEffect(.degrees(-90))
                        Text(String(format: "%d:%02d", (op?.remaining ?? 0) / 60, (op?.remaining ?? 0) % 60))
                            .font(.system(size: 10, weight: .semibold, design: .rounded)).monospacedDigit()
                    }
                    .frame(width: 40, height: 40)
                }
            }
            VStack(alignment: .leading, spacing: 9) {
                stepRow(1, "等回复传完", detail: "Claude Code 已暂停发新请求；拦不住的客户端只能等它们传完。", step: step)
                if phone { stepRow(2, "打开 iPhone 热点", detail: "远程打开，不用碰手机。", step: step) }
                stepRow(3, "加入 \(target)", detail: "现在的 Wi-Fi 会断开几秒。", step: step)
                stepRow(4, "确认连得上", detail: "经代理访问 AI 服务。", step: step)
                stepRow(5, "Claude Code 恢复发请求", detail: nil, step: step)
            }
            if step == "wrong_password", op?.running == false {
                Text("密码改了的话，在切换网络里重新输入即可。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            HStack(spacing: 9) {
                Spacer(minLength: 0)
                if op?.running == true, ["waiting", "waking"].contains(step) {
                    Button("取消切换") { Task { await service.network("cancel") } }.buttonStyle(.pill(.secondary, height: 27))
                    if step == "waiting" {
                        Button("后台等") { close() }.buttonStyle(.pill(.primary, height: 27))
                            .help("关掉这个框，在菜单栏和「网络」页还能看到进度")
                    }
                } else if op?.running == true {
                    Text("几秒就好，这一步不能停").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                } else if step == "wrong_password" {
                    Button("不切了") { close() }.buttonStyle(.pill(.secondary, height: 27))
                    Button("重新输入") { retry(.wifi(target)) }.buttonStyle(.pill(.primary, height: 27))
                } else {
                    Button("好") { close() }.buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                }
            }
            .padding(.top, 3.5)
        }
        .padding(20)
        .frame(width: 408, alignment: .topLeading)
        .background(Palette.card)
    }

    private func title(_ step: String, _ target: String) -> String {
        switch step {
        case "waiting": "等着切到 \(target)"
        case "waking", "joining", "checking": "正在切到 \(target)"
        case "done": "已切到 \(target)"
        case "unreachable": "已切到 \(target)，但连不上"
        default: "没切到 \(target)"
        }
    }

    private func lead(_ op: NetworkStatus.Operation?) -> String {
        guard let op else { return "" }
        switch op.step {
        case "waiting": return "现在切会截断回复，传完就切。"
        case "waking": return "回复都传完了，正在打开热点。"
        case "joining": return "正在加入。"
        case "checking": return "已加入，正在确认。"
        case "done": return (op.waited ?? 0) > 0 ? "等了 \(op.waited ?? 0) 秒，没有截断任何回复。" : "没有截断任何回复。"
        default: return [op.message, "Claude Code 已恢复。"].compactMap { $0 }.joined(separator: " ")
        }
    }

    // 1 wait, 2 wake, 3 join, 4 check, 5 resume.
    private func stepRow(_ index: Int, _ label: String, detail: String?, step: String) -> some View {
        let at: Int = switch step {
        case "waiting", "timeout", "cancelled": 1
        case "waking": 2
        case "joining", "wrong_password", "not_found", "failed": 3
        case "checking", "unreachable": 4
        default: 6
        }
        let failed = ["timeout", "cancelled", "wrong_password", "not_found", "failed", "unreachable"].contains(step)
        let done = index < at
        let bad = failed && index == at
        let current = !failed && index == at
        return HStack(alignment: .top, spacing: 10) {
            ZStack {
                if done {
                    Circle().fill(Palette.solid)
                    Icon(.check, size: 9, color: .white, weight: 2.6)
                } else if bad {
                    Circle().fill(Palette.attentionFill)
                    Text("!").font(.system(size: 10, weight: .bold)).foregroundStyle(Palette.attention)
                } else if current && index < 5 {
                    ProgressView().controlSize(.mini)
                } else {
                    Circle().strokeBorder(Palette.dashStrong, lineWidth: 1.5)
                }
            }
            .frame(width: 17, height: 17)
            VStack(alignment: .leading, spacing: 1) {
                Text(label).font(.system(size: 12, weight: current || bad ? .semibold : .regular))
                    .foregroundStyle(done || current || bad ? Palette.ink : Palette.ink3)
                if current, let detail {
                    Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}
