import SwiftUI
import LiteMailCore

struct SidebarView: View {
    @Bindable var model: AppModel
    @AppStorage("collapsedAccounts") private var collapsedRaw = ""
    @Environment(\.openSettings) private var openSettings

    private var collapsed: Set<Int64> {
        Set(collapsedRaw.split(separator: ",").compactMap { Int64($0) })
    }

    func toggle(_ id: Int64) {
        var c = collapsed
        if c.contains(id) { c.remove(id) } else { c.insert(id) }
        collapsedRaw = c.map(String.init).joined(separator: ",")
        // A selected folder that disappears would leave the list showing nothing.
        if case .folder(let f)? = model.sidebarSelection, model.folder(f)?.accountId == id, c.contains(id) {
            model.sidebarSelection = .smart(.allInboxes)
        }
    }

    private func unread(_ accountId: Int64) -> Int {
        Int(model.folders[accountId]?.first { $0.role == .inbox }?.unread ?? 0)
    }

    private func showAccountSettings() {
        UserDefaults.standard.set("accounts", forKey: "settingsTab")
        openSettings()
    }

    var body: some View {
        List(selection: $model.sidebarSelection) {
            Section {
                ForEach(SmartBox.allCases, id: \.self) { box in
                    Label(box.title, systemImage: box.symbol)
                        .badge(badge(for: box))
                        .tag(SidebarItem.smart(box))
                        .contextMenu {
                            if box != .flagged {
                                Button("Mark All as Read") {
                                    model.markAllSeen(box == .unread ? .unread : .allInboxes)
                                }
                            }
                        }
                }
            }
            // Account header as a plain row with our own fold control: the system's
            // Section(isExpanded:) chevron didn't respond to clicks here.
            ForEach(model.accounts, id: \.id) { account in
                AccountHeader(
                    account: account,
                    phase: model.phases[account.id],
                    expanded: !collapsed.contains(account.id),
                    unread: unread(account.id)
                ) {
                    withAnimation(.snappy(duration: 0.22)) { toggle(account.id) }
                }
                .selectionDisabled()
                .contextMenu {
                    Button(collapsed.contains(account.id) ? String(localized: "Show Folders") : String(localized: "Hide Folders")) { toggle(account.id) }
                    Button("Sync Now") { model.syncNow(account.id) }
                    Button("Account Settings…") { showAccountSettings() }
                }
                if !collapsed.contains(account.id) {
                    ForEach(model.folders[account.id] ?? [], id: \.id) { folder in
                        FolderRow(folder: folder)
                            .tag(SidebarItem.folder(folder.id))
                            .contextMenu { folderMenu(folder) }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Theme.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            StatusBar(model: model, openAccountSettings: showAccountSettings)
        }
    }

    private func badge(for box: SmartBox) -> Int {
        switch box {
        case .allInboxes, .unread: return Int(model.counts.inboxUnread)
        case .flagged: return Int(model.counts.flagged)
        }
    }

    @ViewBuilder
    private func folderMenu(_ folder: Folder) -> some View {
        Button("Mark All as Read") { model.markAllSeen(.folder(id: folder.id)) }
            .disabled(folder.unread == 0)
        Button("Sync Now") { model.syncNow(folder.accountId) }
        Divider()
        Toggle("Keep on This Mac", isOn: Binding(
            get: { folder.syncEnabled },
            set: { on in
                do { try model.core?.setFolderSync(folderId: folder.id, enabled: on) } catch {
                    model.banner = Banner(text: describe(error))
                }
            }
        ))
    }
}

private struct FolderRow: View {
    let folder: Folder

    var body: some View {
        Label {
            HStack(spacing: 4) {
                Text(folder.displayName)
                if !folder.syncEnabled {
                    Image(systemName: "icloud")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .help(Text("Not kept on this Mac. Right-click to turn on."))
                }
            }
        } icon: {
            Image(systemName: folder.role.symbol)
        }
        .padding(.leading, CGFloat(folder.depth) * 12)
        .foregroundStyle(folder.syncEnabled ? .primary : .secondary)
        .badge(folder.role == .inbox || folder.role == .custom ? Int(folder.unread) : 0)
    }
}

private struct AccountHeader: View {
    let account: Account
    let phase: SyncPhase?
    let expanded: Bool
    /// Inbox unread, shown while folded so nothing hides behind the fold.
    let unread: Int
    let toggle: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 6) {
                Circle().fill(Palette.accountDot(account.email)).frame(width: 7, height: 7)
                Text(account.label)
                    .font(.system(size: 11.5, weight: .semibold))
                    .foregroundStyle(Theme.ink3)
                    .lineLimit(1)
                // Problems only; activity shows in the status bar.
                switch phase {
                case .offline(let message)?:
                    Image(systemName: "wifi.slash").font(.system(size: 10.5)).foregroundStyle(Theme.ink3).help(Text("Offline: \(message)"))
                case .authFailed(let message)?:
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 10.5))
                        .foregroundStyle(Theme.flag)
                        .help(Text("Sign-in failed: \(message). Update the password in Settings."))
                default:
                    EmptyView()
                }
                Spacer(minLength: 4)
                if !expanded && unread > 0 {
                    Text(verbatim: "\(unread)")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Theme.ink3)
                        .monospacedDigit()
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 9.5, weight: .bold))
                    .foregroundStyle(hover ? Theme.ink2 : Theme.ink3)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .frame(width: 14, height: 14)
            }
            .padding(.top, 10)
            .padding(.bottom, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(expanded ? Text("Hide folders") : Text("Show folders"))
        .accessibilityLabel(Text(account.label))
        .accessibilityValue(expanded ? Text("Expanded") : Text("Collapsed"))
    }
}

/// Bottom of the sidebar: one line of sync status and the Get New Mail button. Click the status
/// for what's kept on this Mac.
struct StatusBar: View {
    @Bindable var model: AppModel
    let openAccountSettings: () -> Void
    @State private var showDetails = false

    private enum Status {
        case downloading(label: String, done: UInt32, total: UInt32)
        case checking
        case needsPassword(String)
        case offline
        case updated(Int64)
    }

    private var status: Status {
        let many = model.accounts.count > 1
        for a in model.accounts {
            if case .syncing(_, let done, let total)? = model.phases[a.id], total > 0 {
                return .downloading(label: many ? a.label : "", done: done, total: total)
            }
        }
        for a in model.accounts {
            if case .authFailed? = model.phases[a.id] { return .needsPassword(a.label) }
        }
        let busy = model.accounts.contains { a in
            switch model.phases[a.id] {
            case .connecting?, .checking?, .syncing?: return true
            default: return false
            }
        }
        if busy || model.refreshNudge { return .checking }
        if model.accounts.contains(where: { if case .offline? = model.phases[$0.id] { return true }; return false }) {
            return .offline
        }
        let last = model.accounts.compactMap { a -> Int64? in
            if case .idle(let t)? = model.phases[a.id], t > 0 { return t }
            return nil
        }.min() ?? 0
        return .updated(last)
    }

    private var busy: Bool {
        switch status {
        case .checking, .downloading: return true
        default: return false
        }
    }

    var body: some View {
        if !model.accounts.isEmpty {
            VStack(spacing: 0) {
                if case .downloading(_, let done, let total) = status {
                    ProgressView(value: Double(done), total: Double(max(total, 1)))
                        .progressViewStyle(.linear)
                        .controlSize(.mini)
                        .tint(Theme.accent)
                        .padding(.horizontal, 12)
                } else {
                    Rectangle().fill(Theme.line).frame(height: 0.5).padding(.horizontal, 12)
                }
                HStack(spacing: 6) {
                    Button {
                        if case .needsPassword = status { openAccountSettings() } else { showDetails.toggle() }
                    } label: {
                        line.contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .popover(isPresented: $showDetails, arrowEdge: .top) { details }
                    Spacer(minLength: 4)
                    RefreshButton(busy: busy) { model.syncNow() }
                }
                .padding(.leading, 16)
                .padding(.trailing, 10)
                .padding(.vertical, 7)
            }
            .background(Theme.sidebar)
        }
    }

    @ViewBuilder
    private var line: some View {
        switch status {
        case .downloading(let label, let done, let total):
            // The account name goes in the tooltip: an address would push the numbers out of view.
            statusText(String(localized: "Downloading \(Int(done)) of \(Int(total))"), Theme.ink2)
                .help(Text(label.isEmpty ? "" : label))
        case .checking:
            statusText(String(localized: "Checking for new mail…"), Theme.ink2)
        case .needsPassword(let label):
            HStack(spacing: 5) {
                Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 11))
                statusText(String(localized: "\(label) needs its password"), Theme.flag)
            }
            .foregroundStyle(Theme.flag)
            .help(Text("Click to enter it in Settings"))
        case .offline:
            HStack(spacing: 5) {
                Image(systemName: "wifi.slash").font(.system(size: 11))
                statusText(String(localized: "Offline · will retry"), Theme.flag)
            }
            .foregroundStyle(Theme.flag)
        case .updated(let ts):
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                statusText(Dates.updated(ts, now: ctx.date), Theme.ink3)
            }
            .help(Text("What’s kept on this Mac"))
        }
    }

    private func statusText(_ s: String, _ color: Color) -> some View {
        Text(s)
            .font(.system(size: 12))
            .foregroundStyle(color)
            .lineLimit(1)
            .truncationMode(.middle)
    }

    private var details: some View {
        let days = Set(model.accounts.map(\.syncDays))
        let headline = days.count == 1 ? SyncDays.sentence(days.first!) : String(localized: "Keeping mail per account")
        return VStack(alignment: .leading, spacing: 6) {
            Text(headline).font(Theme.serif(16, .medium))
            Text("\(Sizes.count(model.storage.messages)) messages · \(Sizes.human(model.storage.bytes)) on this Mac")
                .font(.system(size: 12))
                .foregroundStyle(Theme.ink2)
            Text("Older mail stays on the server — search finds it there.")
                .font(.system(size: 12))
                .foregroundStyle(Theme.ink3)
                .fixedSize(horizontal: false, vertical: true)
            Button("Account Settings…") {
                showDetails = false
                openAccountSettings()
            }
            .buttonStyle(.link)
            .tint(Theme.accentInk)
            .padding(.top, 4)
        }
        .padding(16)
        .frame(width: 260, alignment: .leading)
    }
}

/// Get New Mail. Spins while mail is being checked.
private struct RefreshButton: View {
    let busy: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "arrow.clockwise")
                .font(.system(size: 12, weight: .semibold))
                .symbolEffect(.rotate, options: .repeat(.continuous), isActive: busy)
                .foregroundStyle(busy ? Theme.accent : Theme.ink2)
                .frame(width: 26, height: 26)
                .background(Circle().fill(hover ? Theme.field : Color.clear))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help(Text("Get new mail (⇧⌘N)"))
        .accessibilityLabel(Text("Get New Mail"))
    }
}
