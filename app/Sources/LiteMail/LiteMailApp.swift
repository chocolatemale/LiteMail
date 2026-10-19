import SwiftUI
import LiteMailCore

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {
        Theme.registerFonts()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        Notifier.shared.setUp()
        MainActor.assumeIsolated { Automation.start() }
        NSApp.registerServicesMenuSendTypes([], returnTypes: [])
    }

    /// Mail keeps syncing and notifying with the window closed.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationWillTerminate(_ notification: Notification) {
        MainActor.assumeIsolated { AppModel.shared.shutdown() }
    }
}

@main
struct LiteMailApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel.shared

    var body: some Scene {
        Window("LiteMail", id: "main") {
            MainView(model: model)
        }
        .defaultSize(width: 1240, height: 780)
        .commands { MailCommands(model: model) }

        WindowGroup("New Message", id: "compose", for: ComposeRequest.self) { $request in
            if let request {
                ComposeView(request: request, model: model)
            }
        }
        .defaultSize(width: 760, height: 640)
        .commandsRemoved()

        Settings {
            SettingsView(model: model)
        }
    }
}

struct MailCommands: Commands {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    private var single: Int64? { model.selectedIDs.count == 1 ? model.selectedIDs.first : nil }
    private var any: Bool { !model.selectedIDs.isEmpty }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Message") {
                openWindow(value: ComposeRequest(kind: .new, accountId: model.detail?.accountId))
            }
            .keyboardShortcut("n")
            .disabled(model.accounts.isEmpty)
            Button("Add Account…") { model.presentingAddAccount = true }
        }
        CommandMenu("Message") {
            Button("Reply") { if let id = single { openWindow(value: ComposeRequest(kind: .reply, messageId: id)) } }
                .keyboardShortcut("r")
                .disabled(single == nil)
            Button("Reply All") { if let id = single { openWindow(value: ComposeRequest(kind: .replyAll, messageId: id)) } }
                .keyboardShortcut("r", modifiers: [.command, .shift])
                .disabled(single == nil)
            Button("Forward") { if let id = single { openWindow(value: ComposeRequest(kind: .forward, messageId: id)) } }
                .keyboardShortcut("f", modifiers: [.command, .shift])
                .disabled(single == nil)
            Divider()
            Button("Mark as Read / Unread") { model.toggleSeen(model.selectedIDs) }
                .keyboardShortcut("u", modifiers: [.command, .shift])
                .disabled(!any)
            Button("Flag / Unflag") { model.toggleFlag(model.selectedIDs) }
                .keyboardShortcut("l", modifiers: [.command, .shift])
                .disabled(!any)
            Button("Archive") { model.archive(model.selectedIDs) }
                .keyboardShortcut("a", modifiers: [.command, .control])
                .disabled(!any)
            Button("Junk / Not Junk") { model.junk(model.selectedIDs) }
                .keyboardShortcut("j", modifiers: [.command, .shift])
                .disabled(!any)
            Button("Move to Trash") { model.delete(model.selectedIDs) }
                .keyboardShortcut(.delete, modifiers: .command)
                .disabled(!any)
            Divider()
            Button("Get All New Mail") { model.syncNow() }
                .keyboardShortcut("n", modifiers: [.command, .shift])
                .disabled(model.accounts.isEmpty)
            Button("View Source") { if let id = single { model.showSource(id) } }
                .keyboardShortcut("u", modifiers: [.command, .option])
                .disabled(single == nil)
            Button("Load Remote Images") { if let id = single { model.allowRemote(for: id) } }
                .disabled(single == nil || model.detail?.hasRemoteContent != true)
        }
        CommandGroup(after: .toolbar) {
            Button("Make Text Bigger") { model.readerZoom = min(model.readerZoom + 0.1, 2.0) }
                .keyboardShortcut("+")
            Button("Make Text Smaller") { model.readerZoom = max(model.readerZoom - 0.1, 0.7) }
                .keyboardShortcut("-")
            Button("Actual Size") { model.readerZoom = 1.0 }
                .keyboardShortcut("0")
            Divider()
        }
        CommandGroup(after: .sidebar) {
            Button("All Inboxes") { model.sidebarSelection = .smart(.allInboxes) }
                .keyboardShortcut("1")
            Button("Unread") { model.sidebarSelection = .smart(.unread) }
                .keyboardShortcut("2")
            Button("Flagged") { model.sidebarSelection = .smart(.flagged) }
                .keyboardShortcut("3")
        }
    }
}
