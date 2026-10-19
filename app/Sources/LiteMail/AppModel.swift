import AppKit
import Foundation
import LiteMailCore
import Network
import Observation

enum SmartBox: String, CaseIterable, Hashable, Codable {
    case allInboxes, unread, flagged

    var title: String {
        switch self {
        case .allInboxes: return String(localized: "All Inboxes")
        case .unread: return String(localized: "Unread")
        case .flagged: return String(localized: "Flagged")
        }
    }

    var symbol: String {
        switch self {
        case .allInboxes: return "tray.2"
        case .unread: return "envelope.badge"
        case .flagged: return "flag"
        }
    }
}

enum SidebarItem: Hashable, Codable {
    case smart(SmartBox)
    case folder(Int64)
}

enum SearchScope: Hashable {
    case everything, current
}

struct Banner: Equatable, Identifiable {
    let id = UUID()
    let text: String
    var isError = true
}

/// Requests for compose windows.
struct ComposeRequest: Codable, Hashable {
    enum Kind: String, Codable { case new, reply, replyAll, forward, mailto, editDraft }
    var kind: Kind
    var messageId: Int64? = nil
    var accountId: Int64? = nil
    var to: String? = nil
    var token = UUID()
}

/// Bridges core events (background threads) to the main actor.
final class CoreBridge: CoreListener, @unchecked Sendable {
    weak var model: AppModel?

    func onEvent(event: CoreEvent) {
        Task { @MainActor [weak model] in model?.handle(event) }
    }
}

@MainActor
@Observable
final class AppModel {
    static let shared = AppModel()

    private(set) var core: MailCore?
    var startupError: String?

    var accounts: [Account] = []
    var folders: [Int64: [Folder]] = [:]
    var phases: [Int64: SyncPhase] = [:]
    var counts = SmartCounts(inboxUnread: 0, flagged: 0)
    var storage = StorageInfo(messages: 0, bytes: 0)

    var sidebarSelection: SidebarItem? = .smart(.allInboxes) {
        didSet { if oldValue != sidebarSelection { mailboxChanged() } }
    }
    var search = "" {
        didSet { if oldValue != search { scheduleReload(resetPaging: true); scheduleServerSearch() } }
    }
    var searchScope: SearchScope = .everything {
        didSet { if oldValue != searchScope { scheduleReload(resetPaging: true); scheduleServerSearch() } }
    }

    var messages: [MessageSummary] = []
    var canLoadMore = false
    var selectedIDs: Set<Int64> = [] {
        didSet { if oldValue != selectedIDs { selectionChanged() } }
    }
    var detail: MessageDetail?
    var detailError: String?
    /// A server search result is being downloaded for the reader.
    var detailLoading = false
    /// Search results for mail older than the local window (negative ids, headers only).
    var serverResults: [MessageSummary] = []
    var serverSearch: ServerSearchState = .idle
    private(set) var refreshNudge = false
    private var nudgeTask: Task<Void, Never>?
    private var serverTask: Task<Void, Never>?
    var remoteAllowed: Set<Int64> = []
    var banner: Banner?
    var presentingAddAccount = false
    /// Compose request issued by the automation harness (tests only).
    var automationCompose: ComposeRequest?
    var automationSettings = false
    var sourceText: String?
    var readerZoom: Double = UserDefaults.standard.object(forKey: "readerZoom") as? Double ?? 1.0 {
        didSet { UserDefaults.standard.set(readerZoom, forKey: "readerZoom") }
    }

    private let bridge = CoreBridge()
    private var reloadTask: Task<Void, Never>?
    private(set) var reloadPending = false
    private var seenTask: Task<Void, Never>?
    private var pathMonitor: NWPathMonitor?
    private let pageSize: UInt32 = 200

    private init() {
        bridge.model = self
        let dir = Self.defaultDataDirectory
        setLanguage(tag: appLanguageTag)
        setHour24(on: !(DateFormatter.dateFormat(fromTemplate: "j", options: 0, locale: .current) ?? "").contains("a"))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            core = try MailCore(dataDir: dir.path, listener: bridge)
        } catch {
            startupError = String(localized: "Couldn’t open the local mail store: \(describe(error))")
        }
        reloadAccounts()
        reloadMessages()
        presentingAddAccount = core != nil && accounts.isEmpty
        observeSystem()
    }

    /// ~/Library/Application Support/LiteMail, or LITEMAIL_DATA_DIR (separate test profiles).
    static var defaultDataDirectory: URL {
        if let custom = ProcessInfo.processInfo.environment["LITEMAIL_DATA_DIR"], !custom.isEmpty {
            return URL(fileURLWithPath: custom, isDirectory: true)
        }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("LiteMail", isDirectory: true)
    }

    var dataDirectory: URL { Self.defaultDataDirectory }

    // MARK: - Events

    func handle(_ event: CoreEvent) {
        switch event {
        case .accountsChanged:
            reloadAccounts()
            scheduleReload(resetPaging: false)
        case .foldersChanged(let accountId):
            folders[accountId] = core?.folders(accountId: accountId) ?? []
            refreshCounts()
        case .messagesChanged:
            scheduleReload(resetPaging: false)
            refreshCounts()
        case .syncState(let accountId, let phase):
            phases[accountId] = phase
            if case .idle = phase { refreshCounts() }
        case .newMail(let accountId, let count, let messageId, let from, let subject):
            Notifier.shared.post(accountId: accountId, count: count, messageId: messageId, from: from, subject: subject)
        }
    }

    // MARK: - Loading

    func reloadAccounts() {
        guard let core else { return }
        accounts = core.accounts()
        var f: [Int64: [Folder]] = [:]
        for a in accounts { f[a.id] = core.folders(accountId: a.id) }
        folders = f
        for a in accounts where phases[a.id] == nil { phases[a.id] = core.syncPhase(accountId: a.id) }
        if case .folder(let id)? = sidebarSelection, folder(id) == nil { sidebarSelection = .smart(.allInboxes) }
        refreshCounts()
    }

    func refreshCounts() {
        guard let core else { return }
        counts = core.smartCounts()
        storage = core.storage(accountId: nil)
        if UserDefaults.standard.object(forKey: "dockBadge") as? Bool ?? true {
            NSApp?.dockTile.badgeLabel = counts.inboxUnread > 0 ? "\(counts.inboxUnread)" : nil
        } else {
            NSApp?.dockTile.badgeLabel = nil
        }
    }

    var currentMailbox: Mailbox {
        switch sidebarSelection {
        case .folder(let id): return .folder(id: id)
        case .smart(.unread): return .unread
        case .smart(.flagged): return .flagged
        default: return .allInboxes
        }
    }

    private var queryMailbox: Mailbox {
        let searching = !search.trimmingCharacters(in: .whitespaces).isEmpty
        return searching && searchScope == .everything ? .everything : currentMailbox
    }

    /// Typing in search debounces (each keystroke restarts the wait). Sync events throttle instead:
    /// a reload already pending is kept, so a long sync that reports every few milliseconds can't
    /// keep pushing the reload back and leave the list empty.
    func scheduleReload(resetPaging: Bool) {
        if !resetPaging, reloadPending { return }
        reloadTask?.cancel()
        reloadPending = true
        reloadTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(resetPaging ? 160 : 120))
            guard !Task.isCancelled, let self else { return }
            self.reloadPending = false
            self.reloadMessages(resetPaging: resetPaging)
        }
    }

    func reloadMessages(resetPaging: Bool = false) {
        guard let core else { return }
        let want = resetPaging ? pageSize : max(pageSize, UInt32(messages.count))
        let q = search.trimmingCharacters(in: .whitespaces)
        let list = core.messages(query: MessageQuery(mailbox: queryMailbox, search: q.isEmpty ? nil : q, limit: want, beforeDate: nil, beforeId: nil))
        messages = list
        canLoadMore = list.count >= Int(want)
        let ids = Set(list.map(\.id)).union(serverResults.map(\.id))
        let kept = selectedIDs.intersection(ids)
        if kept != selectedIDs { selectedIDs = kept }
        if let d = detail, selectedIDs.count == 1, selectedIDs.contains(d.id),
           let s = list.first(where: { $0.id == d.id }), s.seen != d.seen || s.flagged != d.flagged {
            detail?.seen = s.seen
            detail?.flagged = s.flagged
        }
    }

    func loadMore() {
        guard let core, canLoadMore, let last = messages.last else { return }
        let q = search.trimmingCharacters(in: .whitespaces)
        let more = core.messages(query: MessageQuery(mailbox: queryMailbox, search: q.isEmpty ? nil : q, limit: pageSize, beforeDate: last.date, beforeId: last.id))
        messages.append(contentsOf: more)
        canLoadMore = more.count >= Int(pageSize)
    }

    private func mailboxChanged() {
        selectedIDs = []
        detail = nil
        reloadMessages(resetPaging: true)
        if isSearching && searchScope == .current { scheduleServerSearch() }
    }

    private func selectionChanged() {
        seenTask?.cancel()
        guard selectedIDs.count == 1, let id = selectedIDs.first else {
            detail = nil
            detailError = nil
            detailLoading = false
            return
        }
        loadDetail(id)
        if let m = messages.first(where: { $0.id == id }), !m.seen {
            seenTask = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(700))
                guard !Task.isCancelled, let self, self.selectedIDs == [id] else { return }
                self.markSeen([id], true)
            }
        }
    }

    func loadDetail(_ id: Int64) {
        guard let core else { return }
        let alwaysRemote = UserDefaults.standard.bool(forKey: "loadRemoteImages")
        if id < 0 {
            // On the server only: download it into memory off the main thread.
            let allow = alwaysRemote || remoteAllowed.contains(id)
            if detail?.id != id { detail = nil }
            detailError = nil
            detailLoading = true
            Task {
                let result = await Task.detached { Result { try core.message(messageId: id, allowRemote: allow) } }.value
                guard self.selectedIDs == [id] else { return }
                self.detailLoading = false
                switch result {
                case .success(let d): self.detail = d
                case .failure(let e): self.detailError = describe(e)
                }
            }
            return
        }
        detailLoading = false
        do {
            detail = try core.message(messageId: id, allowRemote: alwaysRemote || remoteAllowed.contains(id))
            detailError = nil
        } catch {
            detail = nil
            detailError = describe(error)
        }
    }

    // MARK: - Lookup

    func folder(_ id: Int64) -> Folder? {
        for list in folders.values { if let f = list.first(where: { $0.id == id }) { return f } }
        return nil
    }

    func account(_ id: Int64) -> Account? { accounts.first { $0.id == id } }

    /// The account a new message is sent from: the open folder's, else the open message's.
    var composeAccountId: Int64? {
        if case .folder(let id)? = sidebarSelection { return folder(id)?.accountId }
        if let d = detail { return d.accountId }
        return accounts.first?.id
    }

    var title: String {
        switch sidebarSelection {
        case .smart(let s): return s.title
        case .folder(let id):
            guard let f = folder(id) else { return String(localized: "Mail") }
            return f.role == .inbox && accounts.count > 1 ? "\(account(f.accountId)?.label ?? "") · \(f.displayName)" : f.displayName
        case nil: return String(localized: "Mail")
        }
    }

    var subtitle: String {
        if accounts.isEmpty { return "" }
        if isSearching {
            let here = messages.count
            let server = serverResults.count
            switch (here, server, serverSearch) {
            case (0, 0, .searching): return String(localized: "Searching the server…")
            case (0, 0, _): return String(localized: "No results")
            case (_, 0, _): return String(localized: "\(Int(here))\(canLoadMore ? "+" : "") on this Mac")
            case (0, _, _): return String(localized: "\(Int(server)) on the server")
            default: return String(localized: "\(Int(here))\(canLoadMore ? "+" : "") on this Mac · \(Int(server)) on the server")
            }
        }
        // Counts come from the store, not from the page of messages loaded into the list.
        let unread: Int
        switch sidebarSelection {
        case .smart(.flagged)?: return String(localized: "\(Int(counts.flagged)) messages")
        case .folder(let id)?:
            guard let f = folder(id) else { return "" }
            // Unread only means something for the inbox and your own folders.
            guard f.role == .inbox || f.role == .custom else {
                return f.total == 0 ? String(localized: "No messages") : String(localized: "\(Int(f.total)) messages")
            }
            unread = Int(f.unread)
        default: unread = Int(counts.inboxUnread)
        }
        return unread > 0 ? String(localized: "\(unread) unread") : String(localized: "All read")
    }

    /// "本机保存到 9月18日" for the end of the list, when every relevant account uses a window.
    var horizonText: String? {
        let relevant: [Account]
        switch sidebarSelection {
        case .folder(let id): relevant = accounts.filter { $0.id == folder(id)?.accountId }
        default: relevant = accounts
        }
        guard !relevant.isEmpty else { return nil }
        if relevant.contains(where: { $0.syncDays == 0 }) { return String(localized: "That’s everything kept on this Mac") }
        let days = relevant.map(\.syncDays).max() ?? 30
        let edge = Calendar.current.date(byAdding: .day, value: -Int(days), to: Date()) ?? Date()
        return String(localized: "Kept on this Mac since \(Dates.monthDay(edge)) · older mail stays on the server")
    }

    var selectedMessages: [MessageSummary] { messages.filter { selectedIDs.contains($0.id) } }

    // MARK: - Actions

    private func run(_ body: () throws -> Void) {
        do { try body() } catch { banner = Banner(text: describe(error)) }
    }

    /// Searches the servers for older mail 0.8 s after typing stops (only accounts that keep a
    /// window of days; the rest have everything locally).
    func scheduleServerSearch() {
        serverTask?.cancel()
        let q = search.trimmingCharacters(in: .whitespaces)
        guard let core, q.count >= 2, accounts.contains(where: { $0.syncDays > 0 }) else {
            serverResults = []
            serverSearch = .idle
            return
        }
        let mailbox = searchScope == .everything ? Mailbox.everything : currentMailbox
        serverSearch = .searching
        serverTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(800))
            guard !Task.isCancelled else { return }
            let result = await Task.detached { Result { try core.searchServer(query: q, mailbox: mailbox) } }.value
            guard !Task.isCancelled, let self, self.search.trimmingCharacters(in: .whitespaces) == q else { return }
            switch result {
            case .success(let list):
                self.serverResults = list
                self.serverSearch = .done
            case .failure(let e):
                self.serverResults = []
                self.serverSearch = .failed(describe(e))
            }
        }
    }

    var isSearching: Bool { !search.trimmingCharacters(in: .whitespaces).isEmpty }

    /// After removing messages from view, select the next one like Mail does.
    private func selectNext(after ids: Set<Int64>) {
        guard let idx = messages.firstIndex(where: { ids.contains($0.id) }) else { return }
        let rest = messages[idx...].first { !ids.contains($0.id) } ?? messages[..<idx].last { !ids.contains($0.id) }
        messages.removeAll { ids.contains($0.id) }
        selectedIDs = rest.map { [$0.id] } ?? []
    }

    func markSeen(_ ids: [Int64], _ seen: Bool) {
        guard let core, !ids.isEmpty else { return }
        run { try core.markSeen(ids: ids, seen: seen) }
        for i in messages.indices where ids.contains(messages[i].id) { messages[i].seen = seen }
        if let d = detail, ids.contains(d.id) { detail?.seen = seen }
        refreshCounts()
    }

    func toggleSeen(_ ids: Set<Int64>) {
        let list = messages.filter { ids.contains($0.id) }
        let target = list.contains { !$0.seen }
        markSeen(list.map(\.id), target)
    }

    func toggleFlag(_ ids: Set<Int64>) {
        guard let core else { return }
        let list = messages.filter { ids.contains($0.id) }
        let target = list.contains { !$0.flagged }
        run { try core.markFlagged(ids: list.map(\.id), flagged: target) }
        for i in messages.indices where ids.contains(messages[i].id) { messages[i].flagged = target }
        if let d = detail, ids.contains(d.id) { detail?.flagged = target }
        refreshCounts()
    }

    func archive(_ ids: Set<Int64>) {
        let ids = ids.filter { $0 > 0 }  // server search results are read-only
        guard let core, !ids.isEmpty else { return }
        do {
            try core.archive(ids: Array(ids))
            selectNext(after: ids)
        } catch { banner = Banner(text: describe(error)) }
    }

    func delete(_ ids: Set<Int64>) {
        let ids = ids.filter { $0 > 0 }  // server search results are read-only
        guard let core, !ids.isEmpty else { return }
        do {
            try core.deleteMessages(ids: Array(ids))
            selectNext(after: ids)
        } catch { banner = Banner(text: describe(error)) }
    }

    func junk(_ ids: Set<Int64>) {
        let ids = ids.filter { $0 > 0 }  // server search results are read-only
        guard let core, !ids.isEmpty else { return }
        let inJunk = ids.allSatisfy { id in
            guard let m = messages.first(where: { $0.id == id }) else { return false }
            return folder(m.folderId)?.role == .junk
        }
        do {
            try core.markJunk(ids: Array(ids), junk: !inJunk)
            selectNext(after: ids)
        } catch { banner = Banner(text: describe(error)) }
    }

    func move(_ ids: Set<Int64>, to folderId: Int64) {
        let ids = ids.filter { $0 > 0 }  // server search results are read-only
        guard let core, !ids.isEmpty else { return }
        do {
            try core.moveMessages(ids: Array(ids), targetFolderId: folderId)
            selectNext(after: ids)
        } catch { banner = Banner(text: describe(error)) }
    }

    func markAllSeen(_ mailbox: Mailbox) {
        guard let core else { return }
        run { try core.markAllSeen(mailbox: mailbox) }
        reloadMessages()
    }

    func allowRemote(for id: Int64) {
        remoteAllowed.insert(id)
        if selectedIDs == [id] { loadDetail(id) }
    }

    func showSource(_ id: Int64) {
        guard let core else { return }
        do { sourceText = try core.messageSource(messageId: id) } catch { banner = Banner(text: describe(error)) }
    }

    func openAttachment(messageId: Int64, index: UInt32) {
        guard let core else { return }
        do {
            let path = try core.attachmentTempFile(messageId: messageId, index: index)
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        } catch { banner = Banner(text: describe(error)) }
    }

    func saveAttachment(messageId: Int64, index: UInt32) {
        guard let core, let d = detail, let att = d.attachments.first(where: { $0.index == index }) else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = att.filename
        panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            try core.saveAttachment(messageId: messageId, index: index, path: url.path)
            banner = Banner(text: String(localized: "Saved \(url.lastPathComponent)"), isError: false)
        } catch { banner = Banner(text: describe(error)) }
    }

    /// Get New Mail (button, ⇧⌘N, account menu). The status bar shows "Checking…" for at least
    /// a moment even when the server answers instantly, so a refresh never looks like a no-op.
    func syncNow(_ accountId: Int64? = nil) {
        core?.syncNow(accountId: accountId)
        refreshNudge = true
        nudgeTask?.cancel()
        nudgeTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(900))
            guard !Task.isCancelled else { return }
            self?.refreshNudge = false
        }
    }

    /// Jump to a message (from a notification).
    func reveal(messageId: Int64) {
        NSApp.activate()
        if sidebarSelection != .smart(.allInboxes) { sidebarSelection = .smart(.allInboxes) }
        search = ""
        reloadMessages(resetPaging: true)
        selectedIDs = [messageId]
    }

    // MARK: - System

    private func observeSystem() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { _ in
            Task { @MainActor in AppModel.shared.core?.networkChanged() }
        }
        let monitor = NWPathMonitor()
        var lastStatus: NWPath.Status?
        monitor.pathUpdateHandler = { path in
            let status = path.status
            Task { @MainActor in
                defer { lastStatus = status }
                if status == .satisfied, lastStatus != nil, lastStatus != .satisfied {
                    AppModel.shared.core?.networkChanged()
                }
            }
        }
        monitor.start(queue: DispatchQueue(label: "litemail.path"))
        pathMonitor = monitor
    }

    func shutdown() {
        core?.shutdown()
    }
}

enum ServerSearchState: Equatable {
    case idle, searching, done
    case failed(String)
}
