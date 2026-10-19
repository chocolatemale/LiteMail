import SwiftUI
import LiteMailCore

struct SettingsView: View {
    @Bindable var model: AppModel
    @AppStorage("settingsTab") private var tab = "general"

    var body: some View {
        TabView(selection: $tab) {
            Tab("General", systemImage: "gearshape", value: "general") { GeneralSettings(model: model) }
            Tab("Accounts", systemImage: "at", value: "accounts") { AccountsSettings(model: model) }
        }
        .tint(Theme.accent)
        .frame(width: 760, height: 600)
    }
}

private struct GeneralSettings: View {
    @Bindable var model: AppModel
    @AppStorage("notifications") private var notifications = true
    @AppStorage("dockBadge") private var dockBadge = true
    @AppStorage("loadRemoteImages") private var loadRemote = false

    var body: some View {
        Form {
            Section("New Mail") {
                Toggle("Show notifications", isOn: $notifications)
                Toggle("Show unread count on the Dock icon", isOn: $dockBadge)
                    .onChange(of: dockBadge) { _, _ in model.refreshCounts() }
            }
            Section {
                Toggle("Load remote images automatically", isOn: $loadRemote)
                Picker("Text size", selection: $model.readerZoom) {
                    Text("Small").tag(0.9)
                    Text("Standard").tag(1.0)
                    Text("Large").tag(1.12)
                    Text("Extra Large").tag(1.25)
                }
            } header: {
                Text("Reading")
            } footer: {
                Text("Remote images are often used to track whether you opened a message. With this off, you can still load them for any single message.")
                    .foregroundStyle(Theme.ink2)
            }
            Section("On This Mac") {
                LabeledContent("Stored", value: String(localized: "\(Sizes.count(model.storage.messages)) messages · \(Sizes.human(model.storage.bytes))"))
                LabeledContent("Location") {
                    Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([model.dataDirectory]) }
                }
                LabeledContent("Version", value: model.core?.version() ?? "—")
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.background)
    }
}

private struct AccountsSettings: View {
    @Bindable var model: AppModel
    @State private var selected: Int64?
    @State private var confirmRemove = false

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                List(selection: $selected) {
                    ForEach(model.accounts, id: \.id) { a in
                        HStack(spacing: 10) {
                            Avatar(name: a.displayName, address: a.email, size: 28)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(a.label).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                                Text(a.email).font(.system(size: 11.5)).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                        .padding(.vertical, 3)
                        .tag(a.id)
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                HStack(spacing: 0) {
                    Button { model.presentingAddAccount = true } label: {
                        Image(systemName: "plus").frame(width: 26, height: 22)
                    }
                    Button { confirmRemove = true } label: {
                        Image(systemName: "minus").frame(width: 26, height: 22)
                    }
                    .disabled(selected == nil)
                    Spacer()
                }
                .buttonStyle(.borderless)
                .padding(8)
            }
            .frame(width: 210)
            .background(Theme.sidebar)
            Divider()
            if let id = selected, let account = model.account(id) {
                AccountDetail(model: model, account: account)
                    .id(account.id)
            } else {
                ContentUnavailableView("Select an Account", systemImage: "at")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear { if selected == nil { selected = model.accounts.first?.id } }
        .onChange(of: model.accounts.map(\.id)) { _, ids in
            if let s = selected, !ids.contains(s) { selected = ids.first }
            if selected == nil { selected = ids.first }
        }
        .confirmationDialog(
            Text("Remove this account?"),
            isPresented: $confirmRemove,
            presenting: selected.flatMap { model.account($0) }
        ) { a in
            Button("Remove \(a.email)", role: .destructive) {
                do { try model.core?.removeAccount(accountId: a.id) } catch {
                    model.banner = Banner(text: describe(error))
                }
                model.reloadAccounts()
            }
        } message: { _ in
            Text("This removes the mail kept on this Mac and the password in the keychain. Nothing on the server changes.")
        }
        .sheet(isPresented: $model.presentingAddAccount) { AddAccountView(model: model) }
    }
}

private struct AccountDetail: View {
    @Bindable var model: AppModel
    let account: Account

    @State private var displayName = ""
    @State private var days: UInt32 = 30
    @State private var customDays = false
    @State private var interval: UInt32 = 5
    @State private var notify = true
    @State private var signature = ""
    @State private var updatingPassword = false
    @State private var saveTask: Task<Void, Never>?
    @State private var loaded = false
    @State private var senders: [SendAddress] = []
    @State private var newAddress = ""
    @State private var newName = ""

    private var folders: [Folder] { model.folders[account.id] ?? [] }
    private var usage: StorageInfo { model.core?.storage(accountId: account.id) ?? StorageInfo(messages: 0, bytes: 0) }

    var body: some View {
        Form {
            Section {
                Picker("Keep", selection: Binding(
                    get: { customDays ? UInt32.max : days },
                    set: { v in
                        if v == UInt32.max {
                            customDays = true
                            if SyncDays.presets.contains(days) { days = days == 0 ? 180 : max(1, days) }
                        } else {
                            customDays = false
                            days = v
                        }
                        save()
                    }
                )) {
                    ForEach(SyncDays.presets, id: \.self) { Text(SyncDays.label($0)).tag($0) }
                    Text("Custom").tag(UInt32.max)
                }
                .pickerStyle(.segmented)
                if customDays {
                    LabeledContent("Keep the last") {
                        HStack(spacing: 6) {
                            TextField("", value: $days, format: .number)
                                .frame(width: 60)
                                .multilineTextAlignment(.trailing)
                                .onSubmit { save() }
                            Stepper("", value: $days, in: 1...3650).labelsHidden()
                            Text("days of mail and attachments")
                        }
                    }
                    .onChange(of: days) { _, _ in save(debounce: true) }
                }
                LabeledContent("Stored", value: String(localized: "\(Sizes.count(usage.messages)) messages · \(Sizes.human(usage.bytes))"))
            } header: {
                Text("Keep on This Mac").font(Theme.serif(15, .medium))
            } footer: {
                Text(days == 0
                    ? String(localized: "This Mac keeps all of this account’s mail and attachments.")
                    : String(localized: "This Mac always keeps every message and attachment from the last \(Int(days)) days, readable and searchable offline. Shorter windows free space right away and never touch the server; longer ones fill in on the next sync."))
                    .foregroundStyle(Theme.ink2)
            }

            Section {
                ForEach(folders, id: \.id) { f in
                    Toggle(isOn: Binding(
                        get: { f.syncEnabled },
                        set: { on in
                            do { try model.core?.setFolderSync(folderId: f.id, enabled: on) } catch {
                                model.banner = Banner(text: describe(error))
                            }
                        }
                    )) {
                        HStack {
                            Label(f.path, systemImage: f.role.symbol)
                            Spacer()
                            if f.syncEnabled { Text("\(f.total)").foregroundStyle(Theme.ink2).monospacedDigit() }
                        }
                    }
                    .help(f.role == .all || f.role == .flagged || f.role == .important ? Text("A Gmail view that repeats other folders, so it isn’t kept by default") : Text(""))
                }
            } header: {
                Text("Folders").font(Theme.serif(15, .medium))
            }

            Section {
                TextField("Your name", text: $displayName)
                    .onSubmit { save() }
                    .onChange(of: displayName) { _, _ in save(debounce: true) }
                LabeledContent("Email", value: account.email)
                LabeledContent("Incoming server", value: "\(account.imap.host):\(account.imap.port)")
                LabeledContent("Outgoing server", value: "\(account.smtp.host):\(account.smtp.port)")
                Picker("Check for new mail", selection: $interval) {
                    Text("Push + every minute").tag(UInt32(1))
                    Text("Push + every 5 minutes").tag(UInt32(5))
                    Text("Push + every 15 minutes").tag(UInt32(15))
                    Text("Push + every 30 minutes").tag(UInt32(30))
                }
                .onChange(of: interval) { _, _ in save() }
                Toggle("Notifications", isOn: $notify)
                    .onChange(of: notify) { _, _ in save() }
                LabeledContent("Password") {
                    Button("Update Password…") { updatingPassword = true }
                }
            } header: {
                Text("Account").font(Theme.serif(15, .medium))
            }

            Section {
                ForEach(senders, id: \.address) { a in
                    HStack(spacing: 8) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(a.address)
                            if !a.name.isEmpty && a.name != displayName {
                                Text(a.name).font(.caption).foregroundStyle(Theme.ink2)
                            }
                        }
                        Spacer()
                        switch a.kind {
                        case .primary:
                            Text("Main").font(.caption).foregroundStyle(Theme.ink3)
                        case .found:
                            Text("Found").font(.caption).foregroundStyle(Theme.ink3)
                                .help(Text("You’ve sent from it before, or it comes with your iCloud address."))
                        case .added:
                            Button {
                                run { try model.core?.removeSendAddress(accountId: account.id, address: a.address) }
                            } label: {
                                Image(systemName: "minus.circle.fill")
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(Theme.ink3)
                            .help(Text("Remove"))
                        }
                    }
                }
                HStack(spacing: 8) {
                    TextField("", text: $newAddress, prompt: Text("Another address you send from"))
                        .labelsHidden()
                        .onSubmit(addSender)
                    TextField("", text: $newName, prompt: Text("Name (optional)"))
                        .labelsHidden()
                        .frame(maxWidth: 150)
                        .onSubmit(addSender)
                    Button("Add", action: addSender)
                        .disabled(!newAddress.contains("@"))
                }
            } header: {
                Text("Send From").font(Theme.serif(15, .medium))
            } footer: {
                Text("iCloud doesn’t tell apps about your other addresses. Add the aliases or custom-domain addresses you use; LiteMail also finds the ones you’ve sent from before. Replies go out from the address a message was sent to.")
                    .foregroundStyle(Theme.ink2)
            }

            Section {
                TextEditor(text: $signature)
                    .font(.system(size: 13))
                    .frame(minHeight: 64)
                    .onChange(of: signature) { _, _ in save(debounce: true) }
            } header: {
                Text("Signature").font(Theme.serif(15, .medium))
            } footer: {
                Text("Added to new messages, replies and forwards.").foregroundStyle(Theme.ink2)
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.background)
        .onAppear(perform: load)
        .sheet(isPresented: $updatingPassword) { PasswordSheet(model: model, account: account) }
    }

    private func load() {
        displayName = account.displayName
        days = account.syncDays
        customDays = !SyncDays.presets.contains(account.syncDays)
        interval = account.syncIntervalMinutes
        notify = account.notify
        signature = account.signature
        reloadSenders()
        DispatchQueue.main.async { loaded = true }
    }

    private func reloadSenders() {
        senders = model.core?.sendAddresses(accountId: account.id) ?? []
    }

    private func addSender() {
        let address = newAddress.trimmingCharacters(in: .whitespaces)
        guard !address.isEmpty else { return }
        run {
            try model.core?.addSendAddress(accountId: account.id, name: newName, address: address)
            newAddress = ""
            newName = ""
        }
    }

    private func run(_ body: () throws -> Void) {
        do { try body() } catch { model.banner = Banner(text: describe(error)) }
        reloadSenders()
    }

    private func save(debounce: Bool = false) {
        guard loaded else { return }
        saveTask?.cancel()
        let settings = AccountSettings(
            displayName: displayName,
            syncDays: days,
            syncIntervalMinutes: interval,
            signature: signature,
            notify: notify
        )
        let id = account.id
        saveTask = Task {
            if debounce { try? await Task.sleep(for: .milliseconds(700)) }
            guard !Task.isCancelled else { return }
            do { try model.core?.updateAccount(accountId: id, settings: settings) } catch {
                model.banner = Banner(text: describe(error))
            }
        }
    }
}

private struct PasswordSheet: View {
    @Bindable var model: AppModel
    let account: Account
    @Environment(\.dismiss) private var dismiss
    @State private var password = ""
    @State private var working = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Update the password for \(account.email)").font(Theme.serif(18, .medium))
            (account.provider == .other ? Text("Enter the new password.") : Text("Enter a newly generated app-specific password."))
                .foregroundStyle(Theme.ink2)
            SecureField("Password", text: $password)
                .textFieldStyle(.roundedBorder)
                .onSubmit { save() }
            if let error { Text(error).font(.callout).foregroundStyle(Theme.accentInk) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(action: save) {
                    if working { ProgressView().controlSize(.small) } else { Text("Verify and Save") }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(password.isEmpty || working)
            }
        }
        .padding(24)
        .frame(width: 420)
    }

    private func save() {
        guard let core = model.core, !password.isEmpty else { return }
        working = true
        error = nil
        let id = account.id
        let pw = password.replacingOccurrences(of: " ", with: "")
        Task {
            let result: Result<Void, Error> = await Task.detached {
                do { try core.updatePassword(accountId: id, password: pw); return .success(()) } catch { return .failure(error) }
            }.value
            working = false
            switch result {
            case .success: dismiss()
            case .failure(let e): error = describe(e)
            }
        }
    }
}
