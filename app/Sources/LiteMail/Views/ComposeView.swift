import SwiftUI
import UniformTypeIdentifiers
import LiteMailCore

struct ComposeView: View {
    let request: ComposeRequest
    @Bindable var model: AppModel
    @Environment(\.dismiss) private var dismiss

    @State private var base: Draft?
    @State private var accountId: Int64 = 0
    /// Every address the accounts can send from (aliases too) and the chosen one ("id|address").
    @State private var senders: [SendAddress] = []
    @State private var fromKey = ""
    @State private var to = ""
    @State private var cc = ""
    @State private var bcc = ""
    @State private var subject = ""
    @State private var text = ""
    @State private var showCopies = false
    @State private var files: [URL] = []
    @State private var carried: [AttachmentInfo] = []
    @State private var keepCarried = true
    @State private var sending = false
    @State private var error: String?
    @State private var importing = false
    @State private var selection: TextSelection?
    @State private var finished = false
    @State private var snapshot = ""
    @State private var suggestions: [EmailAddress] = []
    @FocusState private var focus: Field?

    enum Field: Hashable { case to, cc, bcc, subject, body }

    var body: some View {
        VStack(spacing: 0) {
            if let error {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    Text(error).font(.callout).textSelection(.enabled)
                    Spacer()
                    Button { self.error = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 9)
                .background(Theme.accent.opacity(0.1))
            }
            header
            editor
            if !files.isEmpty || (keepCarried && !carried.isEmpty) {
                attachmentBar
            }
        }
        .navigationTitle(subject.isEmpty ? String(localized: "New Message") : subject)
        .background(Theme.paper)
        .tint(Theme.accent)
        .toolbar { toolbar }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { add(urls) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            add(urls.filter(\.isFileURL))
            return true
        }
        .disabled(sending)
        .onAppear(perform: load)
        .onDisappear(perform: autosave)
        .onChange(of: accountId) { old, new in swapSignature(from: old, to: new) }
        .frame(minWidth: 560, minHeight: 420)
    }

    // MARK: Header fields

    private var header: some View {
        VStack(spacing: 0) {
            row(String(localized: "From")) {
                Picker("From", selection: $fromKey) {
                    ForEach(model.accounts, id: \.id) { a in
                        let mine = senders.filter { $0.accountId == a.id }
                        if model.accounts.count > 1 {
                            Section(a.label) { senderItems(mine) }
                        } else {
                            senderItems(mine)
                        }
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .fixedSize()
                .onChange(of: fromKey) { _, key in
                    if let id = Int64(key.split(separator: "|").first ?? ""), id != accountId { accountId = id }
                }
                Spacer()
                if let a = model.account(accountId), a.label != a.email {
                    Text(a.label).font(.caption).foregroundStyle(.tertiary)
                }
            }
            row(String(localized: "To")) {
                addressField($to, .to)
                if !showCopies {
                    Button("Cc / Bcc") { withAnimation { showCopies = true } }
                        .buttonStyle(.plain)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
            suggestionList(for: .to)
            if showCopies {
                row(String(localized: "Cc")) { addressField($cc, .cc) }
                suggestionList(for: .cc)
                row(String(localized: "Bcc")) { addressField($bcc, .bcc) }
                suggestionList(for: .bcc)
            }
            row(String(localized: "Subject")) {
                TextField("Subject", text: $subject)
                    .textFieldStyle(.plain)
                    .labelsHidden()
                    .focused($focus, equals: .subject)
                    .onSubmit { focus = .body }
            }
        }
    }

    private func row<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(label)
                    .foregroundStyle(Theme.ink3)
                    .frame(width: 58, alignment: .leading)
                content()
            }
            .font(.system(size: 13))
            .frame(minHeight: 40)
            .padding(.horizontal, 24)
            Divider().padding(.leading, 24)
        }
    }

    private func addressField(_ text: Binding<String>, _ field: Field) -> some View {
        TextField("", text: text, prompt: field == .to ? Text("Names or addresses, separated by commas") : Text(verbatim: ""))
            .textFieldStyle(.plain)
            .labelsHidden()
            .focused($focus, equals: field)
            .onChange(of: text.wrappedValue) { _, v in updateSuggestions(v, field) }
            .onChange(of: focus) { _, f in if f != field { suggestions = [] } }
            .onSubmit {
                if let first = suggestions.first { complete(text, with: first) } else { focus = nextField(after: field) }
            }
    }

    @ViewBuilder
    private func suggestionList(for field: Field) -> some View {
        if focus == field, !suggestions.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(suggestions.enumerated()), id: \.offset) { i, s in
                    Button {
                        complete(binding(for: field), with: s)
                    } label: {
                        HStack(spacing: 10) {
                            Avatar(name: s.name, address: s.address, size: 22)
                            Text(s.name.isEmpty ? s.address : s.name).font(.system(size: 13, weight: .medium))
                            if !s.name.isEmpty { Text(s.address).font(.system(size: 12)).foregroundStyle(.secondary) }
                            Spacer()
                            if i == 0 { Text("↩").font(.caption).foregroundStyle(.tertiary) }
                        }
                        .padding(.horizontal, 24 + 60)
                        .padding(.vertical, 5)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .background(i == 0 ? Theme.accent.opacity(0.1) : .clear)
                }
            }
            .padding(.vertical, 4)
            Divider().padding(.leading, 24)
        }
    }

    private func binding(for field: Field) -> Binding<String> {
        switch field {
        case .cc: return $cc
        case .bcc: return $bcc
        default: return $to
        }
    }

    private func nextField(after f: Field) -> Field {
        switch f {
        case .to: return showCopies ? .cc : .subject
        case .cc: return .bcc
        default: return .subject
        }
    }

    private func lastToken(_ s: String) -> String {
        let parts = s.split(omittingEmptySubsequences: false, whereSeparator: { ",，;；".contains($0) })
        return (parts.last.map(String.init) ?? "").trimmingCharacters(in: .whitespaces)
    }

    private func updateSuggestions(_ text: String, _ field: Field) {
        guard focus == field, let core = model.core else { return }
        let token = lastToken(text)
        guard !token.isEmpty, !token.contains("<") else { suggestions = []; return }
        let existing = (to + "," + cc + "," + bcc).lowercased()
        suggestions = core.contacts(query: token, limit: 6).filter { !existing.contains($0.address.lowercased()) }
    }

    private func complete(_ text: Binding<String>, with s: EmailAddress) {
        var parts = text.wrappedValue.split(whereSeparator: { ",，;；".contains($0) }).map { $0.trimmingCharacters(in: .whitespaces) }
        if !parts.isEmpty { parts.removeLast() }
        let formatted = s.name.isEmpty ? s.address : (s.name.contains(",") ? "\"\(s.name)\" <\(s.address)>" : "\(s.name) <\(s.address)>")
        parts.append(formatted)
        text.wrappedValue = parts.joined(separator: ", ") + ", "
        suggestions = []
    }

    // MARK: Body and attachments

    private var editor: some View {
        TextEditor(text: $text, selection: $selection)
            .font(.system(size: 14))
            .lineSpacing(3)
            .scrollContentBackground(.hidden)
            .padding(.horizontal, 19)
            .padding(.top, 12)
            .focused($focus, equals: .body)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var attachmentBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                ForEach(files, id: \.self) { url in
                    AttachmentChip(name: url.lastPathComponent, size: fileSize(url)) {
                        files.removeAll { $0 == url }
                    }
                }
                if keepCarried {
                    ForEach(carried, id: \.index) { a in
                        AttachmentChip(name: a.filename, size: a.size) { keepCarried = false }
                            .help(Text("From the original message. Removing it removes all of the original attachments."))
                    }
                }
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 12)
        }
        .background(Theme.field)
        .overlay(alignment: .top) { Divider() }
    }

    private func fileSize(_ url: URL) -> UInt64 {
        (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize).flatMap { $0 }.map(UInt64.init) ?? 0
    }

    private func add(_ urls: [URL]) {
        for u in urls where !files.contains(u) {
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: u.path, isDirectory: &isDir), !isDir.boolValue {
                files.append(u)
            }
        }
    }

    // MARK: Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Button(role: .destructive) {
                finished = true
                dismiss()
            } label: { Label("Discard", systemImage: "trash") }
            .help(Text("Discard this message"))
        }
        ToolbarItem {
            Button { importing = true } label: { Label("Attach Files", systemImage: "paperclip") }
                .help(Text("Attach files (⇧⌘A)"))
                .keyboardShortcut("a", modifiers: [.command, .shift])
        }
        ToolbarSpacer(.fixed)
        ToolbarItem {
            Button(action: send) {
                if sending {
                    ProgressView().controlSize(.small)
                } else {
                    Label("Send", systemImage: "paperplane.fill")
                }
            }
            .buttonStyle(.glassProminent)
            .tint(Theme.accent)
            .keyboardShortcut(.return, modifiers: .command)
            .help(Text("Send (⌘↩)"))
            .disabled(sending || (to + cc + bcc).trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    @ViewBuilder
    private func senderItems(_ list: [SendAddress]) -> some View {
        ForEach(list, id: \.address) { s in
            Text(s.name.isEmpty ? s.address : "\(s.name) <\(s.address)>").tag(Self.key(s.accountId, s.address))
        }
    }

    private static func key(_ accountId: Int64, _ address: String) -> String { "\(accountId)|\(address.lowercased())" }

    // MARK: Lifecycle

    private func load() {
        guard base == nil, let core = model.core else { return }
        let fallback = request.accountId ?? model.accounts.first?.id ?? 0
        do {
            var d: Draft
            switch request.kind {
            case .new, .mailto: d = try core.newDraft(accountId: fallback)
            case .reply: d = try core.replyDraft(messageId: request.messageId ?? 0, all: false)
            case .replyAll: d = try core.replyDraft(messageId: request.messageId ?? 0, all: true)
            case .forward: d = try core.forwardDraft(messageId: request.messageId ?? 0)
            case .editDraft: d = try core.editDraft(messageId: request.messageId ?? 0)
            }
            if request.kind == .mailto, let s = request.to { applyMailto(s, to: &d) }
            base = d
            accountId = d.accountId
            senders = model.accounts.flatMap { core.sendAddresses(accountId: $0.id) }
            let wanted = Self.key(d.accountId, d.from?.address ?? model.account(d.accountId)?.email ?? "")
            fromKey = senders.contains { Self.key($0.accountId, $0.address) == wanted }
                ? wanted
                : Self.key(d.accountId, model.account(d.accountId)?.email ?? "")
            to = d.to
            cc = d.cc
            bcc = d.bcc
            subject = d.subject
            text = d.body
            showCopies = !d.cc.isEmpty || !d.bcc.isEmpty
            if let src = d.forwardFrom, let detail = try? core.message(messageId: src, allowRemote: false) {
                carried = detail.attachments
            }
            snapshot = current
            selection = TextSelection(insertionPoint: text.startIndex)
            focus = to.isEmpty ? .to : .body
        } catch {
            self.error = describe(error)
        }
    }

    private var current: String { [to, cc, bcc, subject, text, files.map(\.path).joined()].joined(separator: "\u{1}") }

    private func applyMailto(_ s: String, to d: inout Draft) {
        guard let c = URLComponents(string: s) else { return }
        d.to = c.path
        for item in c.queryItems ?? [] {
            switch item.name.lowercased() {
            case "subject": d.subject = item.value ?? ""
            case "cc": d.cc = item.value ?? ""
            case "bcc": d.bcc = item.value ?? ""
            case "body": d.body = (item.value ?? "") + d.body
            default: break
            }
        }
    }

    private func signatureBlock(_ id: Int64) -> String {
        let sig = model.account(id)?.signature.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return sig.isEmpty ? "" : "\n\n-- \n\(sig)"
    }

    private func swapSignature(from old: Int64, to new: Int64) {
        guard old != 0, old != new else { return }
        let oldSig = signatureBlock(old)
        if !oldSig.isEmpty, let r = text.range(of: oldSig) {
            text.replaceSubrange(r, with: signatureBlock(new))
        } else if oldSig.isEmpty, text.isEmpty {
            text = signatureBlock(new)
        }
    }

    /// The chosen sender when it isn't the account's own address.
    private var chosenSender: EmailAddress? {
        guard let s = senders.first(where: { Self.key($0.accountId, $0.address) == fromKey }), s.kind != .primary else { return nil }
        return EmailAddress(name: s.name, address: s.address)
    }

    private func makeDraft() -> Draft? {
        guard let b = base else { return nil }
        return Draft(
            accountId: accountId,
            to: to,
            cc: cc,
            bcc: bcc,
            subject: subject,
            body: text,
            attachments: files.map(\.path),
            inReplyTo: b.inReplyTo,
            references: b.references,
            replyToMessage: b.replyToMessage,
            forwardFrom: keepCarried ? b.forwardFrom : nil,
            draftSource: b.draftSource,
            from: chosenSender
        )
    }

    private func send() {
        guard let core = model.core, let draft = makeDraft() else { return }
        sending = true
        error = nil
        Task {
            let result: Result<Void, Error> = await Task.detached {
                do { try core.send(draft: draft); return .success(()) } catch { return .failure(error) }
            }.value
            sending = false
            switch result {
            case .success:
                finished = true
                model.banner = Banner(text: String(localized: "Sent"), isError: false)
                dismiss()
            case .failure(let e):
                error = (e as? CoreError)?.isAuth == true
                    ? String(localized: "\(describe(e)). Update this account’s password in Settings.")
                    : describe(e)
            }
        }
    }

    /// Closing the window keeps unsent work as a draft on the server.
    private func autosave() {
        guard !finished, base != nil, current != snapshot, let core = model.core, let draft = makeDraft() else { return }
        do {
            try core.saveDraft(draft: draft)
            model.banner = Banner(text: String(localized: "Saved to Drafts"), isError: false)
        } catch {
            model.banner = Banner(text: String(localized: "Couldn’t save the draft: \(describe(error))"))
        }
    }
}

struct AttachmentChip: View {
    let name: String
    let size: UInt64
    let remove: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            let ext = (name as NSString).pathExtension.uppercased()
            Text(ext.isEmpty ? "FILE" : String(ext.prefix(4)))
                .font(.system(size: 9, weight: .bold, design: .monospaced))
                .foregroundStyle(Theme.accentInk)
                .frame(width: 32, height: 32)
                .background(Theme.accent.opacity(0.13), in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 1) {
                Text(name).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                Text(Sizes.human(size)).font(.system(size: 11)).foregroundStyle(.secondary)
            }
            .frame(maxWidth: 180, alignment: .leading)
            Button(action: remove) {
                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .opacity(hovering ? 1 : 0.35)
        }
        .padding(.leading, 9)
        .padding(.trailing, 10)
        .frame(height: 48)
        .background(Theme.paper, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.line, lineWidth: 0.5))
        .onHover { hovering = $0 }
    }
}
