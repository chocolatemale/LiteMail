import AppKit
import WebKit
import LiteMailCore

/// Test-only remote control, active only when LITEMAIL_AUTOMATION=<dir> is set.
/// Write commands (one per line) to <dir>/command.txt; results go to <dir>/result.txt.
///
///   smart all|unread|flagged      folder <name>        message <index>      messages <i> <j> ...
///   search <text>                 scope all|current     appearance light|dark|system
///   compose new|reply|replyAll|forward                  settings             close-compose
///   allow-remote                  add-account <email> <host> <imapPort> <smtpPort> [days]
///   sync                          wait <ms>             snapshot <name>      dump
@MainActor
enum Automation {
    private static var dir: URL?
    private static var busy = false

    static func start() {
        guard let path = ProcessInfo.processInfo.environment["LITEMAIL_AUTOMATION"], !path.isEmpty else { return }
        let url = URL(fileURLWithPath: path, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        dir = url
        Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { _ in
            Task { @MainActor in await poll() }
        }
    }

    private static func poll() async {
        guard let dir, !busy else { return }
        let cmd = dir.appendingPathComponent("command.txt")
        guard let text = try? String(contentsOf: cmd, encoding: .utf8) else { return }
        try? FileManager.default.removeItem(at: cmd)
        busy = true
        var log: [String] = []
        for line in text.split(separator: "\n").map({ $0.trimmingCharacters(in: .whitespaces) }) where !line.isEmpty {
            do {
                log.append("ok   \(line)\(try await run(line).map { " → \($0)" } ?? "")")
            } catch {
                log.append("FAIL \(line) → \(error)")
            }
        }
        try? log.joined(separator: "\n").appending("\n").write(to: dir.appendingPathComponent("result.txt"), atomically: true, encoding: .utf8)
        busy = false
    }

    struct Failure: Error, CustomStringConvertible { let description: String }

    private static func run(_ line: String) async throws -> String? {
        let model = AppModel.shared
        let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
        let arg = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "wait":
            // Seconds, fractions allowed: "wait 2", "wait 0.5".
            try await Task.sleep(for: .seconds(Double(arg) ?? 0.5))
        case "smart":
            model.sidebarSelection = .smart(arg == "unread" ? .unread : arg == "flagged" ? .flagged : .allInboxes)
        case "folder":
            guard let f = model.folders.values.flatMap({ $0 }).first(where: { $0.displayName == arg || $0.path == arg }) else {
                throw Failure(description: "no folder \(arg)")
            }
            model.sidebarSelection = .folder(f.id)
        case "message":
            guard let i = Int(arg), i < model.messages.count else { throw Failure(description: "no message \(arg)") }
            model.selectedIDs = [model.messages[i].id]
            return model.messages[i].subject
        case "fold":
            // Same storage the sidebar's account header toggles.
            var c = Set((UserDefaults.standard.string(forKey: "collapsedAccounts") ?? "").split(separator: ",").map(String.init))
            if c.contains(arg) { c.remove(arg) } else { c.insert(arg) }
            UserDefaults.standard.set(c.sorted().joined(separator: ","), forKey: "collapsedAccounts")
        case "dodge-cursor":
            // Moves the main window off the mouse so no row shows hover controls in screenshots.
            let mouse = NSEvent.mouseLocation
            if let w = NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 600 }), let screen = w.screen ?? NSScreen.main {
                let vf = screen.visibleFrame
                var f = w.frame
                f.origin.x = mouse.x > vf.midX ? vf.minX + 8 : vf.maxX - f.width - 8
                f.origin.y = mouse.y > vf.midY ? vf.minY + 8 : vf.maxY - f.height - 8
                if f.contains(mouse) { f.origin.y = mouse.y > vf.midY ? mouse.y - f.height - 40 : mouse.y + 40 }
                w.setFrame(f, display: true)
            }
        case "senders":
            return (model.core?.sendAddresses(accountId: Int64(arg) ?? 0) ?? []).map { "\($0.address)[\($0.kind)]" }.joined(separator: ", ")
        case "settings-scroll":
            // Scrolls the Settings form so its lower sections can be captured (0 = top, 1 = bottom).
            let f = CGFloat(Double(arg) ?? 1)
            for w in NSApp.windows where w.isVisible && w.identifier?.rawValue.contains("Settings") == true || w.title == "Accounts" || w.title == "账户" {
                let scrolls = Self.all(NSScrollView.self, in: w.contentView).sorted { $0.frame.height > $1.frame.height }
                if let sv = scrolls.first, let doc = sv.documentView {
                    let y = max(0, (doc.frame.height - sv.contentView.bounds.height) * f)
                    sv.contentView.scroll(to: NSPoint(x: 0, y: doc.isFlipped ? y : doc.frame.height - sv.contentView.bounds.height - y))
                    sv.reflectScrolledClipView(sv.contentView)
                }
            }
        case "add-sender":
            let a = arg.split(separator: " ", maxSplits: 1).map(String.init)
            guard a.count == 2, let id = Int64(a[0]) else { throw Failure(description: "usage: add-sender <accountId> <address>") }
            try model.core?.addSendAddress(accountId: id, name: "", address: a[1])
        case "server-message":
            guard let i = Int(arg), i < model.serverResults.count else { throw Failure(description: "no server result \(arg)") }
            model.selectedIDs = [model.serverResults[i].id]
            return model.serverResults[i].subject
        case "messages":
            model.selectedIDs = Set(arg.split(separator: " ").compactMap { Int($0) }.filter { $0 < model.messages.count }.map { model.messages[$0].id })
        case "search":
            model.search = arg
        case "scope":
            model.searchScope = arg == "current" ? .current : .everything
        case "appearance":
            NSApp.appearance = arg == "dark" ? NSAppearance(named: .darkAqua) : arg == "light" ? NSAppearance(named: .aqua) : nil
        case "activate":
            // Brings the window to the front so selections draw in their active colours.
            // `activate()` is cooperative and is declined for a background-launched process; the
            // older call still brings a test run forward.
            NSApp.activate(ignoringOtherApps: true)
            NSApp.windows.first { $0.isVisible && $0.frame.width > 600 }?.makeKeyAndOrderFront(nil)
        case "hide":
            NSApp.hide(nil)
        case "unhide":
            NSApp.unhide(nil)
        case "focus-list":
            // Makes the message list the first responder (selection drawn in the accent colour).
            if let w = NSApp.keyWindow, let table = Self.find(NSTableView.self, in: w.contentView, skip: 1) { w.makeFirstResponder(table) }
        case "compose":
            let kind = ComposeRequest.Kind(rawValue: arg) ?? .new
            model.automationCompose = ComposeRequest(kind: kind, messageId: model.selectedIDs.first, accountId: model.accounts.first?.id)
        case "close-compose":
            for w in NSApp.windows where w.identifier?.rawValue.hasPrefix("compose") == true { w.performClose(nil) }
        case "settings":
            if !arg.isEmpty { UserDefaults.standard.set(arg, forKey: "settingsTab") }
            model.automationSettings = true
        case "allow-remote":
            if let id = model.selectedIDs.first { model.allowRemote(for: id) }
        case "sync":
            model.syncNow()
        case "add-account":
            let a = arg.split(separator: " ").map(String.init)
            guard a.count >= 4, let core = model.core else { throw Failure(description: "usage") }
            let setup = AccountSetup(
                email: a[0], displayName: "Alex", provider: a.count > 5 && a[5] == "icloud" ? .iCloud : .other,
                imap: ServerConfig(host: a[1], port: UInt16(a[2]) ?? 31143, security: .none),
                smtp: ServerConfig(host: a[1], port: UInt16(a[3]) ?? 31587, security: .none),
                username: a[0], password: "test", syncDays: a.count > 4 ? UInt32(a[4]) ?? 30 : 30, allowInvalidCerts: true
            )
            let account = try await Task.detached { try core.addAccount(setup: setup) }.value
            model.presentingAddAccount = false
            model.reloadAccounts()
            return "account \(account.id)"
        case "snapshot":
            return try await snapshot(name: arg)
        case "dump":
            return "accent=\(NSColor.controlAccentColor.usingColorSpace(.sRGB).map { String(format: "%.2f,%.2f,%.2f", $0.redComponent, $0.greenComponent, $0.blueComponent) } ?? "?") pending=\(model.reloadPending) server=\(model.serverSearch)/\(model.serverResults.count) accounts=\(model.accounts.count) messages=\(model.messages.count) selected=\(model.selectedIDs.count) title=\(model.title) subtitle=\(model.subtitle) phases=\(model.phases.values.map { "\($0)" })"
        default:
            throw Failure(description: "unknown command")
        }
        return nil
    }

    /// Captures every visible window of this app (own windows need no screen-recording permission).
    private static func snapshot(name: String) async throws -> String {
        guard let dir else { return "" }
        try await Task.sleep(for: .milliseconds(350))
        var written: [String] = []
        for (i, w) in NSApp.windows.enumerated() where w.isVisible && w.frame.width > 200 {
            guard let image = captureWindow(w) else { continue }
            let suffix = i == 0 ? "" : "-\(w.title.isEmpty ? String(i) : w.title.replacingOccurrences(of: "/", with: "_"))"
            let url = dir.appendingPathComponent("\(name)\(suffix).png")
            let rep = NSBitmapImageRep(cgImage: image)
            try rep.representation(using: .png, properties: [:])?.write(to: url)
            written.append(url.lastPathComponent)
        }
        return written.joined(separator: ", ")
    }

    /// The `skip`-th view of a type in a depth-first walk (0 = first; the sidebar's table comes first).
    private static func find<T: NSView>(_ type: T.Type, in root: NSView?, skip: Int) -> T? {
        var found: [T] = []
        func walk(_ v: NSView) {
            if let t = v as? T { found.append(t) }
            v.subviews.forEach(walk)
        }
        if let root { walk(root) }
        return found.count > skip ? found[skip] : found.last
    }

    private static func all<T: NSView>(_ type: T.Type, in root: NSView?) -> [T] {
        var found: [T] = []
        func walk(_ v: NSView) {
            if let t = v as? T { found.append(t) }
            v.subviews.forEach(walk)
        }
        if let root { walk(root) }
        return found
    }

    private typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?

    private static func captureWindow(_ w: NSWindow) -> CGImage? {
        // CGWindowListCreateImage is obsoleted in the macOS 15 SDK but still works for own windows.
        guard let sym = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let fn = unsafeBitCast(sym, to: CreateImage.self)
        // kCGWindowListOptionIncludingWindow = 1 << 3, kCGWindowImageBoundsIgnoreFraming = 1 << 0 | nominal res 1 << 4
        return fn(.null, 1 << 3, UInt32(w.windowNumber), (1 << 0))?.takeRetainedValue()
    }
}
