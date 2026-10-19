import SwiftUI
import LiteMailCore

struct AddAccountView: View {
    @Bindable var model: AppModel
    @Environment(\.dismiss) private var dismiss

    @State private var kind: ProviderKind = .iCloud
    @State private var email = ""
    @State private var name = NSFullUserName()
    @State private var password = ""
    @State private var days: UInt32 = 30
    @State private var imapHost = ""
    @State private var imapPort = "993"
    @State private var imapSecurity: Security = .tls
    @State private var smtpHost = ""
    @State private var smtpPort = "465"
    @State private var smtpSecurity: Security = .tls
    @State private var username = ""
    @State private var working = false
    @State private var error: String?
    @FocusState private var focus: Field?

    enum Field { case email, name, password }

    private var preset: ProviderPreset? {
        guard let core = model.core else { return nil }
        if kind == .other { return email.contains("@") ? core.suggestSettings(email: email) : nil }
        return core.providers().first { $0.kind == kind }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Add an account").font(Theme.serif(28, .medium))
            Text("LiteMail keeps the days you choose on this Mac; older mail stays on the server. Your password is stored in the macOS keychain.")
                .font(.system(size: 13))
                .foregroundStyle(Theme.ink2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
                .padding(.bottom, 20)

            HStack(spacing: 10) {
                tile(.iCloud, mark: "iC", color: Theme.accent)
                tile(.gmail, mark: "G", color: Color(light: 0x6A9BCC, dark: 0x6A9BCC))
                tile(.other, mark: "@", color: Color(light: 0x8B8A82, dark: 0x6F6E67), note: String(localized: "Fastmail, Yahoo, your own"))
            }
            .padding(.bottom, 20)

            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 14, verticalSpacing: 12) {
                GridRow {
                    Color.clear.frame(width: 118, height: 0)
                    Color.clear.frame(maxWidth: .infinity).frame(height: 0)
                }
                GridRow {
                    label(String(localized: "Email"))
                    TextField("", text: $email, prompt: Text(kind == .gmail ? "name@gmail.com" : kind == .iCloud ? "name@icloud.com" : "name@example.com"))
                        .textContentType(.username)
                        .focused($focus, equals: .email)
                        .onSubmit { focus = .password }
                }
                GridRow {
                    label(String(localized: "Your name"))
                    TextField("", text: $name, prompt: Text("Shown to people you write to"))
                        .focused($focus, equals: .name)
                }
                GridRow {
                    label(kind == .other ? String(localized: "Password") : String(localized: "App password"))
                    SecureField("", text: $password, prompt: Text(kind == .other ? "" : "xxxx-xxxx-xxxx-xxxx"))
                        .textContentType(.password)
                        .focused($focus, equals: .password)
                        .onSubmit(login)
                }
                if let p = preset, !p.passwordHelpText.isEmpty {
                    GridRow {
                        Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: "key").foregroundStyle(Theme.accent).padding(.top, 1)
                            VStack(alignment: .leading, spacing: 4) {
                                Text(p.passwordHelpText).fixedSize(horizontal: false, vertical: true)
                                if let url = URL(string: p.passwordHelpUrl), !p.passwordHelpUrl.isEmpty {
                                    Link("Open the page ↗", destination: url).foregroundStyle(Theme.accentInk)
                                }
                            }
                        }
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.ink2)
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Theme.field, in: RoundedRectangle(cornerRadius: 10))
                    }
                }
                if kind == .other {
                    GridRow {
                        label(String(localized: "Incoming"))
                        serverRow(host: $imapHost, port: $imapPort, security: $imapSecurity, placeholder: "imap.example.com")
                    }
                    GridRow {
                        label(String(localized: "Outgoing"))
                        serverRow(host: $smtpHost, port: $smtpPort, security: $smtpSecurity, placeholder: "smtp.example.com")
                    }
                    GridRow {
                        label(String(localized: "User name"))
                        TextField("", text: $username, prompt: Text("Usually your email address"))
                    }
                }
                GridRow {
                    label(String(localized: "Keep on this Mac"))
                    Picker("Keep on this Mac", selection: $days) {
                        ForEach(SyncDays.presets, id: \.self) { Text(SyncDays.label($0)).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                }
            }
            .textFieldStyle(.roundedBorder)
            .frame(maxWidth: .infinity)

            if let error {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "exclamationmark.circle.fill").foregroundStyle(Theme.accent)
                    Text(error).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                }
                .font(.system(size: 12.5))
                .padding(.top, 16)
            }

            HStack {
                Label(footnote, systemImage: "lock")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(action: login) {
                    if working {
                        ProgressView().controlSize(.small).frame(width: 36)
                    } else {
                        Text("Sign In").frame(minWidth: 44)
                    }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .tint(Theme.accent)
                .disabled(working || email.isEmpty || password.isEmpty)
            }
            .padding(.top, 24)
        }
        .padding(.horizontal, 34)
        .padding(.top, 30)
        .padding(.bottom, 24)
        .frame(width: 600)
        .background(Theme.background)
        .tint(Theme.accent)
        .disabled(working)
        .onAppear { focus = .email }
        .onChange(of: email) { _, _ in fillServers() }
        .onChange(of: kind) { _, _ in fillServers(); error = nil }
    }

    private var footnote: String {
        switch kind {
        case .iCloud: return String(localized: "Connects only to Apple")
        case .gmail: return String(localized: "Connects only to Google")
        case .other: return String(localized: "Connects only to the servers you enter")
        }
    }

    private func label(_ s: String) -> some View {
        Text(s).foregroundStyle(Theme.ink2).frame(width: 118, alignment: .trailing).gridColumnAlignment(.trailing)
    }

    private func tile(_ k: ProviderKind, mark: String, color: Color, note: String? = nil) -> some View {
        Button {
            kind = k
        } label: {
            VStack(spacing: 7) {
                Text(mark)
                    .font(.system(size: 13, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 32, height: 32)
                    .background(color, in: RoundedRectangle(cornerRadius: 9))
                Text(k.title).font(.system(size: 13, weight: .semibold))
                if let note { Text(note).font(.system(size: 11)).foregroundStyle(Theme.ink3) }
            }
            .frame(maxWidth: .infinity)
            .frame(height: 88)
            .background(Theme.paper, in: RoundedRectangle(cornerRadius: 12))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(kind == k ? Theme.accent : Theme.line, lineWidth: kind == k ? 2 : 1)
            )
            .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
    }

    private func serverRow(host: Binding<String>, port: Binding<String>, security: Binding<Security>, placeholder: String) -> some View {
        HStack(spacing: 8) {
            TextField("", text: host, prompt: Text(placeholder))
            TextField("", text: port).frame(width: 56)
            Picker("", selection: security) {
                Text("SSL/TLS").tag(Security.tls)
                Text("STARTTLS").tag(Security.startTls)
                Text("None").tag(Security.none)
            }
            .labelsHidden()
            .frame(width: 104)
        }
    }

    private func fillServers() {
        guard kind == .other, let p = preset else { return }
        imapHost = p.imap.host
        imapPort = String(p.imap.port)
        imapSecurity = p.imap.security
        smtpHost = p.smtp.host
        smtpPort = String(p.smtp.port)
        smtpSecurity = p.smtp.security
        if kind == .other, email.contains("@"), let suggested = model.core?.suggestSettings(email: email), suggested.kind != .other {
            kind = suggested.kind
        }
    }

    private func login() {
        guard let core = model.core, !working else { return }
        let address = email.trimmingCharacters(in: .whitespaces)
        guard address.contains("@") else { error = String(localized: "Enter a full email address"); return }
        let p = kind == .other ? nil : preset
        let setup = AccountSetup(
            email: address,
            displayName: name.trimmingCharacters(in: .whitespaces),
            provider: kind,
            imap: p?.imap ?? ServerConfig(host: imapHost.trimmingCharacters(in: .whitespaces), port: UInt16(imapPort) ?? 993, security: imapSecurity),
            smtp: p?.smtp ?? ServerConfig(host: smtpHost.trimmingCharacters(in: .whitespaces), port: UInt16(smtpPort) ?? 465, security: smtpSecurity),
            username: kind == .other && !username.isEmpty ? username : address,
            password: password.replacingOccurrences(of: " ", with: ""),
            syncDays: days,
            allowInvalidCerts: kind == .other && ["localhost", "127.0.0.1"].contains(imapHost)
        )
        working = true
        error = nil
        Task {
            let result: Result<Account, Error> = await Task.detached {
                do { return .success(try core.addAccount(setup: setup)) } catch { return .failure(error) }
            }.value
            working = false
            switch result {
            case .success:
                Notifier.shared.requestPermission()
                model.reloadAccounts()
                dismiss()
            case .failure(let e):
                var text = describe(e)
                if (e as? CoreError)?.isAuth == true, kind != .other {
                    text += "\n" + String(localized: "Make sure you used an app-specific password, not your account password.")
                }
                error = text
            }
        }
    }
}
