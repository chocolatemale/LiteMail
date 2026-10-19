import AppKit
import UserNotifications

/// New-mail notifications. Only active when running from the .app bundle.
final class Notifier: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let shared = Notifier()

    private var center: UNUserNotificationCenter? {
        Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    }

    func setUp() {
        center?.delegate = self
    }

    func requestPermission() {
        center?.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    func post(accountId: Int64, count: UInt32, messageId: Int64, from: String, subject: String) {
        guard UserDefaults.standard.object(forKey: "notifications") as? Bool ?? true, let center else { return }
        let content = UNMutableNotificationContent()
        content.title = from.isEmpty ? String(localized: "New Mail") : from
        content.body = subject.isEmpty ? String(localized: "(No subject)") : subject
        if count > 1 { content.subtitle = String(localized: "\(Int(count - 1)) more new messages") }
        content.sound = .default
        content.threadIdentifier = "account-\(accountId)"
        content.userInfo = ["messageId": messageId]
        center.add(UNNotificationRequest(identifier: "mail-\(messageId)", content: content, trigger: nil))
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let id = response.notification.request.content.userInfo["messageId"] as? Int64 else { return }
        await MainActor.run { AppModel.shared.reveal(messageId: id) }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        // While the app is frontmost the list updates itself; stay quiet.
        let active = await MainActor.run { NSApp.isActive }
        return active ? [] : [.banner, .sound]
    }
}
