import AppKit
import CoreText
import SwiftUI
import LiteMailCore

extension Color {
    /// A colour that follows the light / dark appearance.
    init(light: UInt32, dark: UInt32) {
        func ns(_ hex: UInt32) -> NSColor {
            NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
                    blue: CGFloat(hex & 0xff) / 255, alpha: 1)
        }
        self.init(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? ns(dark) : ns(light)
        })
    }
}

/// Claude-style palette: warm paper neutrals and one terracotta accent.
enum Theme {
    static let accent = Color(light: 0xD97757, dark: 0xD97757)
    static let accentInk = Color(light: 0xB85A3C, dark: 0xE8957A)
    static let background = Color(light: 0xFAF9F5, dark: 0x262624)
    static let sidebar = Color(light: 0xF0EEE6, dark: 0x1F1E1D)
    static let paper = Color(light: 0xFFFEFB, dark: 0x2B2A27)
    static let field = Color(light: 0xF5F4ED, dark: 0x34332F)
    static let ink = Color(light: 0x141413, dark: 0xF5F4EF)
    static let ink2 = Color(light: 0x5E5D59, dark: 0xB7B5A9)
    static let ink3 = Color(light: 0x9C9A92, dark: 0x85837A)
    static let line = Color(light: 0xE8E6DC, dark: 0x3A3936)
    static let flag = Color(light: 0xC98A2B, dark: 0xE0A64B)

    static let serifFamily = "Source Serif 4"
    private(set) static var serifAvailable = false

    /// Registers the bundled Source Serif 4 (SIL OFL) for this process.
    static func registerFonts() {
        guard let dir = Bundle.main.resourceURL?.appendingPathComponent("Fonts"),
              let urls = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
        else { return }
        let fonts = urls.filter { $0.pathExtension == "ttf" } as CFArray
        CTFontManagerRegisterFontURLs(fonts, .process, true, nil)
        serifAvailable = NSFont(name: "SourceSerif4Roman-Regular", size: 12) != nil
            || NSFontManager.shared.availableMembers(ofFontFamily: serifFamily) != nil
    }

    /// The reading / headline serif (Source Serif 4, falling back to New York).
    static func serif(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        if serifAvailable { return Font.custom(serifFamily, size: size).weight(weight) }
        return Font.system(size: size, weight: weight, design: .serif)
    }

    static func avatarFill(_ hue: Double, dark: Bool) -> Color {
        dark ? Palette.oklch(0.42, 0.06, hue) : Palette.oklch(0.9, 0.045, hue)
    }
}

/// OKLCH → sRGB, and the hue hash shared with the Rust core.
enum Palette {
    static func oklch(_ l: Double, _ c: Double, _ h: Double, opacity: Double = 1) -> Color {
        let hr = h * .pi / 180
        let a = c * cos(hr)
        let b = c * sin(hr)
        let l_ = l + 0.3963377774 * a + 0.2158037573 * b
        let m_ = l - 0.1055613458 * a - 0.0638541728 * b
        let s_ = l - 0.0894841775 * a - 1.2914855480 * b
        let L = l_ * l_ * l_, M = m_ * m_ * m_, S = s_ * s_ * s_
        let r = 4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S
        let g = -1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S
        let bl = -0.0041960863 * L - 0.7034186147 * M + 1.7076147010 * S
        func gamma(_ x: Double) -> Double {
            let v = x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1 / 2.4) - 0.055
            return min(max(v, 0), 1)
        }
        return Color(.sRGB, red: gamma(r), green: gamma(g), blue: gamma(bl), opacity: opacity)
    }

    /// Same FNV-1a hue as the Rust core (`reader::hue`), so avatars match the reading pane.
    static func hue(_ key: String) -> Double {
        var h: UInt32 = 0x811c9dc5
        for b in key.trimmingCharacters(in: .whitespaces).lowercased().utf8 {
            h ^= UInt32(b)
            h = h &* 0x01000193
        }
        return Double(h % 360)
    }

    static func accountDot(_ email: String) -> Color { oklch(0.66, 0.12, hue(email)) }
}

func initials(name: String, address: String) -> String {
    let n = name.trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: "\"'")))
    if let first = n.first {
        if !first.isASCII { return String(first) }
        let letters = n.split(separator: " ").compactMap { $0.first(where: { $0.isLetter || $0.isNumber }) }.prefix(2)
        if !letters.isEmpty { return String(letters).uppercased() }
    }
    if let c = address.first(where: { $0.isLetter || $0.isNumber }) { return String(c).uppercased() }
    return "?"
}

struct Avatar: View {
    let name: String
    let address: String
    var size: CGFloat = 32
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        Circle()
            .fill(Theme.avatarFill(Palette.hue(address), dark: scheme == .dark))
            .overlay(
                Text(initials(name: name, address: address))
                    .font(.system(size: size * 0.37, weight: .semibold))
                    .foregroundStyle(scheme == .dark ? Color(white: 0.96) : Color(red: 0.24, green: 0.165, blue: 0.12))
            )
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

enum Dates {
    static var calendar: Calendar { Calendar.current }

    private static func days(_ d: Date, _ now: Date) -> Int {
        calendar.dateComponents([.day], from: calendar.startOfDay(for: d), to: calendar.startOfDay(for: now)).day ?? 99
    }

    /// Compact time for list rows under a day header: 9:41 AM (today and yesterday — the header
    /// already says which) · Thu · Oct 14 · 10/14/25
    static func short(_ ts: Int64, now: Date = Date(), locale: Locale = .autoupdatingCurrent) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(ts))
        let n = days(d, now)
        if n <= 1 { return d.formatted(Date.FormatStyle(date: .omitted, time: .shortened).locale(locale)) }
        if n < 7 { return d.formatted(Date.FormatStyle().weekday(.abbreviated).locale(locale)) }
        if calendar.component(.year, from: d) == calendar.component(.year, from: now) {
            return d.formatted(Date.FormatStyle().month(.abbreviated).day().locale(locale))
        }
        return d.formatted(Date.FormatStyle(date: .numeric, time: .omitted).locale(locale))
    }

    /// Section header for grouping the list.
    static func section(_ ts: Int64, now: Date = Date(), locale: Locale = .autoupdatingCurrent) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(ts))
        switch days(d, now) {
        case ..<1: return String(localized: "Today", locale: locale)
        case 1: return String(localized: "Yesterday", locale: locale)
        case 2..<7: return String(localized: "This Week", locale: locale)
        default:
            if calendar.component(.year, from: d) == calendar.component(.year, from: now) {
                return d.formatted(Date.FormatStyle().month(.wide).locale(locale))
            }
            return d.formatted(Date.FormatStyle().year().month(.wide).locale(locale))
        }
    }

    static func monthDay(_ date: Date) -> String {
        date.formatted(.dateTime.month(.abbreviated).day())
    }

    /// "Updated just now" / "Updated 5 min ago" / "Updated 2 h ago" / "Updated Oct 18".
    static func updated(_ ts: Int64, now: Date = Date()) -> String {
        guard ts > 0 else { return String(localized: "Not updated yet") }
        let secs = Int(now.timeIntervalSince1970) - Int(ts)
        if secs < 60 { return String(localized: "Updated just now") }
        if secs < 3600 { return String(localized: "Updated \(secs / 60) min ago") }
        if secs < 86400 { return String(localized: "Updated \(secs / 3600) h ago") }
        return String(localized: "Updated \(monthDay(Date(timeIntervalSince1970: TimeInterval(ts))))")
    }
}

enum Sizes {
    static func human(_ bytes: UInt64) -> String {
        let b = Double(bytes)
        if b >= 1_073_741_824 { return String(format: "%.1f GB", b / 1_073_741_824) }
        if b >= 1_048_576 { return String(format: "%.1f MB", b / 1_048_576) }
        if b >= 1024 { return "\(Int((b / 1024).rounded())) KB" }
        return "\(bytes) B"
    }

    static func count(_ n: UInt64) -> String {
        n.formatted(.number.grouping(.automatic))
    }
}

enum SyncDays {
    static let presets: [UInt32] = [7, 30, 90, 365, 0]

    static func label(_ days: UInt32) -> String {
        switch days {
        case 0: return String(localized: "All")
        case 365: return String(localized: "1 year")
        default: return String(localized: "\(Int(days)) days")
        }
    }

    static func sentence(_ days: UInt32) -> String {
        switch days {
        case 0: return String(localized: "Keeping all mail")
        case 365: return String(localized: "Keeping the last year")
        default: return String(localized: "Keeping the last \(Int(days)) days")
        }
    }
}

extension FolderRole {
    var symbol: String {
        switch self {
        case .inbox: return "tray"
        case .sent: return "paperplane"
        case .drafts: return "doc"
        case .archive: return "archivebox"
        case .junk: return "xmark.bin"
        case .trash: return "trash"
        case .all: return "tray.full"
        case .flagged: return "star"
        case .important: return "exclamationmark.circle"
        case .custom: return "folder"
        }
    }
}

extension ProviderKind {
    var title: String {
        switch self {
        case .iCloud: return "iCloud"
        case .gmail: return "Gmail"
        case .other: return String(localized: "Other IMAP")
        }
    }
}

extension Account {
    var label: String {
        switch provider {
        case .iCloud: return "iCloud"
        case .gmail: return "Gmail"
        case .other: return email
        }
    }
}

extension CoreError {
    var message: String {
        switch self {
        case .Network(let m), .Auth(let m), .Server(let m), .InvalidInput(let m), .Storage(let m): return m
        case .NotFound: return String(localized: "Message or account not found")
        }
    }

    var isAuth: Bool {
        if case .Auth = self { return true }
        return false
    }
}

func describe(_ error: Error) -> String {
    (error as? CoreError)?.message ?? error.localizedDescription
}

/// Language tag handed to the Rust core so its strings match the UI.
var appLanguageTag: String {
    Bundle.main.preferredLocalizations.first ?? Locale.preferredLanguages.first ?? "en"
}
