import SwiftUI
import WebKit
import LiteMailCore

struct ReaderColumn: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        content
            .toolbar { toolbar }
    }

    @ViewBuilder
    private var content: some View {
        if let err = model.startupError {
            ContentUnavailableView("LiteMail couldn’t start", systemImage: "exclamationmark.triangle", description: Text(err))
        } else if model.accounts.isEmpty {
            WelcomeView(model: model)
        } else if model.selectedIDs.count > 1 {
            MultiSelectionView(model: model)
        } else if model.detailLoading {
            VStack(spacing: 10) {
                ProgressView()
                Text("Opening from the server…").font(.callout).foregroundStyle(Theme.ink2)
                Text("It stays on the server; LiteMail keeps it in memory only.").font(.caption).foregroundStyle(Theme.ink3)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.paper)
        } else if let d = model.detail {
            MailWebView(
                html: d.html,
                messageId: d.id,
                zoom: model.readerZoom,
                onLink: { handle($0, detail: d, modifiers: $1) }
            )
            .id(d.id)
            .background(Theme.paper)
        } else if let err = model.detailError {
            ContentUnavailableView("Can’t show this message", systemImage: "exclamationmark.triangle", description: Text(err))
        } else {
            ContentUnavailableView {
                Label("No Message Selected", systemImage: "envelope")
            } description: {
                Text("\(Sizes.count(model.storage.messages)) messages kept on this Mac")
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.paper)
        }
    }

    private var single: Int64? { model.selectedIDs.count == 1 ? model.selectedIDs.first : nil }
    /// Archive, delete and flag need mail kept on this Mac; server search results are read-only.
    private var hasSelection: Bool { !model.selectedIDs.isEmpty && model.selectedIDs.allSatisfy { $0 > 0 } }

    // Actions on the open message sit over the reading pane, like Mail; New Message sits over the list.
    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                if let id = single { openWindow(value: ComposeRequest(kind: .reply, messageId: id)) }
            } label: { Label("Reply", systemImage: "arrowshape.turn.up.left") }
                .help(Text("Reply (⌘R)"))
                .disabled(single == nil)
            Button {
                if let id = single { openWindow(value: ComposeRequest(kind: .replyAll, messageId: id)) }
            } label: { Label("Reply All", systemImage: "arrowshape.turn.up.left.2") }
                .help(Text("Reply all (⇧⌘R)"))
                .disabled(single == nil)
            Button {
                if let id = single { openWindow(value: ComposeRequest(kind: .forward, messageId: id)) }
            } label: { Label("Forward", systemImage: "arrowshape.turn.up.right") }
                .help(Text("Forward (⇧⌘F)"))
                .disabled(single == nil)
        }
        ToolbarSpacer(.fixed, placement: .primaryAction)
        ToolbarItemGroup(placement: .primaryAction) {
            Button { model.archive(model.selectedIDs) } label: { Label("Archive", systemImage: "archivebox") }
                .help(Text("Archive (⌃⌘A)"))
                .disabled(!hasSelection)
            Button { model.delete(model.selectedIDs) } label: { Label("Delete", systemImage: "trash") }
                .help(Text("Delete (⌫)"))
                .disabled(!hasSelection)
            Button { model.toggleFlag(model.selectedIDs) } label: {
                Label("Flag", systemImage: model.detail?.flagged == true ? "flag.fill" : "flag")
            }
            .help(Text("Flag or unflag (⇧⌘L)"))
            .disabled(!hasSelection)
        }
    }

    private func handle(_ url: URL, detail d: MessageDetail, modifiers: NSEvent.ModifierFlags) {
        if url.scheme == "litemail" {
            switch url.host() {
            case "attachment":
                guard let idx = UInt32(url.lastPathComponent) else { return }
                if modifiers.contains(.option) {
                    model.saveAttachment(messageId: d.id, index: idx)
                } else {
                    model.openAttachment(messageId: d.id, index: idx)
                }
            case "load-remote":
                model.allowRemote(for: d.id)
            case "unsubscribe":
                guard let target = d.listUnsubscribe, let u = URL(string: target) else { return }
                if u.scheme == "mailto" {
                    openWindow(value: ComposeRequest(kind: .mailto, accountId: d.accountId, to: target))
                } else {
                    NSWorkspace.shared.open(u)
                }
            default:
                break
            }
        } else if url.scheme == "mailto" {
            openWindow(value: ComposeRequest(kind: .mailto, accountId: d.accountId, to: url.absoluteString))
        } else if url.scheme == "http" || url.scheme == "https" {
            NSWorkspace.shared.open(url)
        }
    }
}

/// Read-only, JavaScript-free web view for one message.
struct MailWebView: NSViewRepresentable {
    let html: String
    let messageId: Int64
    let zoom: Double
    let onLink: (URL, NSEvent.ModifierFlags) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onLink: onLink) }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.suppressesIncrementalRendering = true
        config.setURLSchemeHandler(AssetSchemeHandler.shared, forURLScheme: "litemail-asset")
        let web = WKWebView(frame: .zero, configuration: config)
        web.navigationDelegate = context.coordinator
        web.uiDelegate = context.coordinator
        web.allowsMagnification = true
        web.allowsBackForwardNavigationGestures = false
        web.setValue(false, forKey: "drawsBackground")
        web.underPageBackgroundColor = NSColor(Theme.paper)
        web.pageZoom = zoom
        context.coordinator.load(html, in: web)
        return web
    }

    func updateNSView(_ web: WKWebView, context: Context) {
        context.coordinator.onLink = onLink
        if web.pageZoom != zoom { web.pageZoom = zoom }
        if context.coordinator.loaded != html { context.coordinator.load(html, in: web) }
    }

    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        var onLink: (URL, NSEvent.ModifierFlags) -> Void
        var loaded = ""

        init(onLink: @escaping (URL, NSEvent.ModifierFlags) -> Void) {
            self.onLink = onLink
        }

        func load(_ html: String, in web: WKWebView) {
            loaded = html
            web.loadHTMLString(html, baseURL: nil)
        }

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction) async -> WKNavigationActionPolicy {
            // The message itself (loadHTMLString) is the only navigation allowed.
            if action.navigationType == .other, action.targetFrame?.isMainFrame == true,
               action.request.url?.absoluteString == "about:blank" {
                return .allow
            }
            if action.navigationType == .linkActivated, let url = action.request.url {
                let flags = action.modifierFlags
                await MainActor.run { onLink(url, flags) }
            }
            return .cancel
        }

        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
            if let url = action.request.url { onLink(url, action.modifierFlags) }
            return nil
        }
    }
}

struct MultiSelectionView: View {
    @Bindable var model: AppModel

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                ForEach(0..<3) { i in
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Theme.paper)
                        .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                        .frame(width: 150, height: 100)
                        .rotationEffect(.degrees(Double(i - 1) * 5))
                        .offset(y: CGFloat(i) * -4)
                }
                Image(systemName: "envelope.fill").font(.system(size: 30)).foregroundStyle(Theme.accent)
            }
            .padding(.bottom, 6)
            Text("\(model.selectedIDs.count) Messages Selected").font(Theme.serif(22, .medium))
            HStack(spacing: 10) {
                Button("Mark as Read") { model.markSeen(Array(model.selectedIDs), true) }
                Button("Archive") { model.archive(model.selectedIDs) }
                Button("Delete", role: .destructive) { model.delete(model.selectedIDs) }
            }
            .controlSize(.large)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.paper)
    }
}

struct WelcomeView: View {
    @Bindable var model: AppModel

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "envelope.open")
                .font(.system(size: 54, weight: .light))
                .foregroundStyle(Theme.accent)
                .padding(.bottom, 6)
            Text("Welcome to LiteMail")
                .font(Theme.serif(34, .medium))
            Text("A light, native mail app. Add iCloud, Gmail or any IMAP account,\nand choose how many days of mail to keep on this Mac.")
                .multilineTextAlignment(.center)
                .foregroundStyle(Theme.ink2)
            Button("Add Account…") { model.presentingAddAccount = true }
                .buttonStyle(.glassProminent)
                .tint(Theme.accent)
                .controlSize(.large)
                .padding(.top, 8)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.paper)
    }
}

/// Serves bundled fonts to the reading pane (WebKit can't see fonts registered by the app).
final class AssetSchemeHandler: NSObject, WKURLSchemeHandler {
    static let shared = AssetSchemeHandler()

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url, url.host() == "fonts",
              let file = Bundle.main.resourceURL?.appendingPathComponent("Fonts").appendingPathComponent(url.lastPathComponent),
              file.pathExtension == "ttf", let data = try? Data(contentsOf: file)
        else {
            task.didFailWithError(URLError(.fileDoesNotExist))
            return
        }
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: [
            "Content-Type": "font/ttf",
            "Access-Control-Allow-Origin": "*",
            "Cache-Control": "max-age=31536000",
        ])!
        task.didReceive(response)
        task.didReceive(data)
        task.didFinish()
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {}
}
