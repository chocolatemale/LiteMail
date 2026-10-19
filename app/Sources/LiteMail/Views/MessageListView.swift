import SwiftUI
import LiteMailCore

struct MessageListView: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    private struct DayGroup: Identifiable {
        let id: String
        var items: [MessageSummary]
    }

    private var groups: [DayGroup] {
        var out: [DayGroup] = []
        let now = Date()
        for m in model.messages {
            let title = Dates.section(m.date, now: now)
            if out.last?.id == title {
                out[out.count - 1].items.append(m)
            } else {
                out.append(DayGroup(id: title, items: [m]))
            }
        }
        return out
    }

    private var showAccount: Bool {
        guard model.accounts.count > 1 else { return false }
        if case .folder? = model.sidebarSelection, model.search.isEmpty { return false }
        return true
    }

    var body: some View {
        Group {
            if model.accounts.isEmpty {
                Color.clear
            } else if model.messages.isEmpty && !(model.isSearching && model.serverSearch != .idle) {
                emptyState
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                list
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if !model.accounts.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text(model.title)
                        .font(Theme.serif(21, .medium))
                        .lineLimit(1)
                    Text(model.subtitle)
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.ink2)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 6)
                .padding(.bottom, 8)
                .background(Theme.background)
            }
        }
        .background(Theme.background)
        .navigationTitle(model.title)
        .toolbar(removing: .title)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    openWindow(value: ComposeRequest(kind: .new, accountId: model.composeAccountId))
                } label: {
                    Label("New Message", systemImage: "square.and.pencil")
                }
                .help(Text("New message (⌘N)"))
                .disabled(model.accounts.isEmpty)
            }
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        if !model.search.trimmingCharacters(in: .whitespaces).isEmpty {
            ContentUnavailableView.search(text: model.search)
        } else if case .folder(let id)? = model.sidebarSelection, let f = model.folder(id), !f.syncEnabled {
            ContentUnavailableView {
                Label("This folder isn’t kept on this Mac", systemImage: "icloud")
            } description: {
                Text("LiteMail shows the mail it keeps on this Mac.")
            } actions: {
                Button("Keep on This Mac") {
                    try? model.core?.setFolderSync(folderId: id, enabled: true)
                }
            }
        } else if isSyncing {
            ContentUnavailableView {
                Label("Syncing", systemImage: "arrow.triangle.2.circlepath")
            } description: {
                Text("The first sync downloads every message in the window you chose.")
            }
        } else {
            ContentUnavailableView {
                Label(emptyTitle, systemImage: emptySymbol)
            } description: {
                if let h = model.horizonText { Text(h) }
            }
        }
    }

    private var isSyncing: Bool {
        model.phases.values.contains { phase in
            switch phase {
            case .connecting, .checking, .syncing: return true
            default: return false
            }
        }
    }

    private var emptyTitle: String {
        switch model.sidebarSelection {
        case .smart(.unread)?: return String(localized: "No Unread Mail")
        case .smart(.flagged)?: return String(localized: "No Flagged Mail")
        default: return String(localized: "No Mail")
        }
    }

    private var emptySymbol: String {
        switch model.sidebarSelection {
        case .smart(.unread)?: return "envelope.open"
        case .smart(.flagged)?: return "flag.slash"
        default: return "tray"
        }
    }

    private var list: some View {
        List(selection: $model.selectedIDs) {
            if model.isSearching && model.messages.isEmpty {
                ListNote(text: String(localized: "No matches on this Mac"))
            }
            ForEach(groups) { group in
                // A plain row, not a Section header: macOS draws pinned headers on a grey band.
                DayLabel(title: group.id)
                    .listRowSeparator(.hidden)
                    .selectionDisabled()
                ForEach(group.items, id: \.id) { m in
                        MessageRow(message: m, accountDot: showAccount ? model.account(m.accountId).map { Palette.accountDot($0.email) } : nil, model: model)
                            .tag(m.id)
                            .contextMenu { menu(for: m) }
                            .swipeActions(edge: .leading) {
                                Button {
                                    model.toggleSeen([m.id])
                                } label: {
                                    Label(m.seen ? String(localized: "Unread") : String(localized: "Read"), systemImage: m.seen ? "envelope.badge" : "envelope.open")
                                }
                                .tint(Theme.accent)
                            }
                            .swipeActions(edge: .trailing) {
                                Button(role: .destructive) {
                                    model.delete([m.id])
                                } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                                Button {
                                    model.archive([m.id])
                                } label: {
                                    Label("Archive", systemImage: "archivebox")
                                }
                                .tint(Theme.ink2)
                            }
                }
            }
            if model.isSearching && model.serverSearch != .idle {
                serverSection
            }
            if model.canLoadMore {
                HStack {
                    Spacer()
                    ProgressView().controlSize(.small)
                    Spacer()
                }
                .onAppear { model.loadMore() }
            } else if let h = model.horizonText, model.search.isEmpty {
                HorizonEdge(text: h)
                    .listRowSeparator(.hidden)
                    .selectionDisabled()
            }
        }
        .listStyle(.inset)
        .scrollContentBackground(.hidden)
        .onDeleteCommand { model.delete(model.selectedIDs) }
        .contextMenu(forSelectionType: Int64.self) { ids in
            if ids.count > 1 { bulkMenu(ids) }
        } primaryAction: { ids in
            if ids.count == 1, let id = ids.first {
                if model.folder(model.messages.first { $0.id == id }?.folderId ?? 0)?.role == .drafts {
                    openWindow(value: ComposeRequest(kind: .editDraft, messageId: id))
                }
            }
        }
    }

    /// Older mail found on the server: headers only; opening one downloads it into memory.
    @ViewBuilder
    private var serverSection: some View {
        DayLabel(title: String(localized: "On the server"))
            .listRowSeparator(.hidden)
            .selectionDisabled()
        ForEach(model.serverResults, id: \.id) { m in
            MessageRow(message: m, accountDot: showAccount ? model.account(m.accountId).map { Palette.accountDot($0.email) } : nil, model: model)
                .tag(m.id)
                .contextMenu { menu(for: m) }
        }
        switch model.serverSearch {
        case .searching:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Searching older mail on the server…")
            }
            .font(.system(size: 12))
            .foregroundStyle(Theme.ink2)
            .padding(.vertical, 6)
            .listRowSeparator(.hidden)
            .selectionDisabled()
        case .failed(let message):
            ListNote(text: String(localized: "Couldn’t search the server: \(message)"), color: Theme.flag)
        case .done where model.serverResults.isEmpty:
            ListNote(text: String(localized: "No older mail on the server"))
        default:
            EmptyView()
        }
    }

    @ViewBuilder
    private func menu(for m: MessageSummary) -> some View {
        let ids: Set<Int64> = model.selectedIDs.contains(m.id) && model.selectedIDs.count > 1 ? model.selectedIDs : [m.id]
        if m.id < 0 {
            // On the server only: read-only.
            Button("Reply") { openWindow(value: ComposeRequest(kind: .reply, messageId: m.id)) }
            Button("Reply All") { openWindow(value: ComposeRequest(kind: .replyAll, messageId: m.id)) }
            Button("Forward") { openWindow(value: ComposeRequest(kind: .forward, messageId: m.id)) }
            Divider()
            Button("View Source") { model.showSource(m.id) }
        } else if ids.count > 1 {
            bulkMenu(ids)
        } else {
            Button("Reply") { openWindow(value: ComposeRequest(kind: .reply, messageId: m.id)) }
            Button("Reply All") { openWindow(value: ComposeRequest(kind: .replyAll, messageId: m.id)) }
            Button("Forward") { openWindow(value: ComposeRequest(kind: .forward, messageId: m.id)) }
            Divider()
            Button(m.seen ? String(localized: "Mark as Unread") : String(localized: "Mark as Read")) { model.toggleSeen(ids) }
            Button(m.flagged ? String(localized: "Remove Flag") : String(localized: "Flag")) { model.toggleFlag(ids) }
            moveMenu(ids, accountId: m.accountId)
            Button("Archive") { model.archive(ids) }
            Button(model.folder(m.folderId)?.role == .junk ? String(localized: "Not Junk") : String(localized: "Move to Junk")) { model.junk(ids) }
            Divider()
            Button("View Source") { model.showSource(m.id) }
            Button("Delete", role: .destructive) { model.delete(ids) }
        }
    }

    @ViewBuilder
    private func bulkMenu(_ ids: Set<Int64>) -> some View {
        Button("Mark as Read") { model.markSeen(Array(ids), true) }
        Button("Mark as Unread") { model.markSeen(Array(ids), false) }
        Button("Flag / Unflag") { model.toggleFlag(ids) }
        if let acct = model.messages.first(where: { ids.contains($0.id) })?.accountId,
           model.messages.filter({ ids.contains($0.id) }).allSatisfy({ $0.accountId == acct }) {
            moveMenu(ids, accountId: acct)
        }
        Button("Archive \(ids.count) Messages") { model.archive(ids) }
        Button("Delete \(ids.count) Messages", role: .destructive) { model.delete(ids) }
    }

    @ViewBuilder
    private func moveMenu(_ ids: Set<Int64>, accountId: Int64) -> some View {
        Menu("Move To") {
            ForEach(model.folders[accountId] ?? [], id: \.id) { f in
                Button {
                    model.move(ids, to: f.id)
                } label: {
                    Label(f.path, systemImage: f.role.symbol)
                }
            }
        }
    }
}

struct MessageRow: View {
    let message: MessageSummary
    let accountDot: Color?
    @Bindable var model: AppModel
    @State private var hovering = false
    @Environment(\.backgroundProminence) private var prominence

    private var selected: Bool { prominence == .increased }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Avatar(name: person.name, address: person.address, size: 32)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    if let dot = accountDot {
                        Circle().fill(dot).frame(width: 6, height: 6).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
                    }
                    Text(sender)
                        .font(.system(size: 13, weight: message.seen ? .regular : .semibold))
                        .lineLimit(1)
                    if message.hasAttachments {
                        Image(systemName: "paperclip").font(.system(size: 10.5)).foregroundStyle(.secondary)
                    }
                    if message.flagged {
                        Image(systemName: "flag.fill").font(.system(size: 10.5)).foregroundStyle(selected ? .white : Theme.flag)
                    }
                    if message.answered {
                        Image(systemName: "arrowshape.turn.up.left.fill").font(.system(size: 9.5)).foregroundStyle(.secondary)
                    }
                    if message.id < 0 {
                        Image(systemName: "icloud").font(.system(size: 10.5)).foregroundStyle(.secondary)
                            .help(Text("On the server — opening it doesn’t save it to this Mac"))
                    }
                    Spacer(minLength: 4)
                    // Unread: a small dot by the time, inside the row (with the bold sender and subject).
                    if !message.seen {
                        Circle()
                            .fill(selected ? Color.white : Theme.accent)
                            .frame(width: 7, height: 7)
                            .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 0.5 }
                            .opacity(hovering && message.id > 0 ? 0 : 1)
                            .accessibilityLabel(Text("Unread"))
                    }
                    Text(Dates.short(message.date))
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .opacity(hovering && message.id > 0 ? 0 : 1)
                }
                Text(message.subject.isEmpty ? String(localized: "(No subject)") : message.subject)
                    .font(.system(size: 13, weight: message.seen ? .regular : .medium))
                    .lineLimit(1)
                    .foregroundStyle(message.subject.isEmpty || message.seen ? .secondary : .primary)
                if !message.snippet.isEmpty {
                    Text(message.snippet)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .overlay(alignment: .topTrailing) {
            if hovering && message.id > 0 {
                HoverActions(message: message, model: model)
                    .offset(y: 1)
                    .transition(.opacity)
            }
        }
        .onHover { h in withAnimation(.easeOut(duration: 0.12)) { hovering = h } }
        .accessibilityElement(children: .combine)
    }

    /// Mail you sent shows who it went to (name and avatar), not yourself.
    private var outgoing: Bool {
        guard let f = model.folder(message.folderId) else { return false }
        return f.role == .sent || f.role == .drafts
    }

    /// The person this row is about: the sender, or for Sent/Drafts the first recipient.
    private var person: (name: String, address: String) {
        guard outgoing else { return (message.fromName, message.fromAddr) }
        let first = message.toLine.split(separator: ",").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        if let l = first.lastIndex(of: "<"), let r = first.lastIndex(of: ">"), l < r {
            let name = first[..<l].trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            return (name, String(first[first.index(after: l)..<r]))
        }
        return ("", first)
    }

    private var sender: String {
        if outgoing {
            let p = person
            let name = p.name.isEmpty ? p.address : p.name
            return name.isEmpty ? String(localized: "(No recipients)") : String(localized: "To: \(name)")
        }
        return message.fromName.isEmpty ? message.fromAddr : message.fromName
    }
}

private struct HoverActions: View {
    let message: MessageSummary
    @Bindable var model: AppModel

    var body: some View {
        HStack(spacing: 0) {
            button("archivebox", String(localized: "Archive")) { model.archive([message.id]) }
            button(message.seen ? "envelope.badge" : "envelope.open", message.seen ? String(localized: "Mark as Unread") : String(localized: "Mark as Read")) {
                model.toggleSeen([message.id])
            }
            button("trash", String(localized: "Delete")) { model.delete([message.id]) }
        }
        .padding(2)
        .glassEffect(.regular, in: .capsule)
    }

    private func button(_ symbol: String, _ help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .frame(width: 26, height: 22)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

/// A quiet one-line note inside the list ("No matches on this Mac").
private struct ListNote: View {
    let text: String
    var color: Color = Theme.ink3

    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(color)
            .padding(.vertical, 6)
            .listRowSeparator(.hidden)
            .selectionDisabled()
    }
}

/// "TODAY", "YESTERDAY", … above each day's messages.
private struct DayLabel: View {
    let title: String

    var body: some View {
        Text(title.uppercased(with: .current))
            .font(.system(size: 10.5, weight: .semibold))
            .tracking(0.6)
            .foregroundStyle(Theme.ink3)
            .padding(.top, 8)
            .padding(.bottom, -2)
            .accessibilityAddTraits(.isHeader)
    }
}

/// The end of the local window: everything older stays on the server.
struct HorizonEdge: View {
    let text: String

    var body: some View {
        // The text takes the width it needs (wrapping only when the column is really narrow);
        // the rules shrink to short ends instead of squeezing it into a sliver.
        HStack(spacing: 10) {
            Rectangle().fill(Theme.line).frame(minWidth: 12, maxWidth: .infinity, maxHeight: 0.5)
            Text(text)
                .font(Theme.serif(12).italic())
                .foregroundStyle(Theme.ink3)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .layoutPriority(1)
            Rectangle().fill(Theme.line).frame(minWidth: 12, maxWidth: .infinity, maxHeight: 0.5)
        }
        .padding(.vertical, 14)
    }
}
