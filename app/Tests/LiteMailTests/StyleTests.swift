import Testing
import Foundation
import AppKit
@testable import LiteMail

@Suite struct StyleTests {
    @Test func hueMatchesRustCore() {
        // Same vectors as core/src/reader.rs `hue_is_stable`.
        #expect(Palette.hue("zhang@corp.example") == 339)
        #expect(Palette.hue("me@icloud.com") == 98)
        #expect(Palette.hue(" ZHANG@corp.example ") == 339)
        #expect(Palette.hue("") == 61)
    }

    @Test func initialsMatchRustCore() {
        #expect(initials(name: "张三", address: "z@x") == "张")
        #expect(initials(name: "Tim Cook", address: "t@x") == "TC")
        #expect(initials(name: "", address: "bob@x") == "B")
        #expect(initials(name: "\"  \"", address: "") == "?")
    }

    @Test func shortDates() {
        let en = Locale(identifier: "en_US")
        let cal = Calendar.current
        let now = cal.date(from: DateComponents(year: 2026, month: 10, day: 18, hour: 12))!
        func ts(_ d: Int, _ h: Int = 9, month: Int = 10, year: Int = 2026) -> Int64 {
            Int64(cal.date(from: DateComponents(year: year, month: month, day: d, hour: h, minute: 41))!.timeIntervalSince1970)
        }
        #expect(Dates.short(ts(18), now: now, locale: en) == "9:41\u{202F}AM")
        #expect(Dates.short(ts(17, 21), now: now, locale: en) == "9:41\u{202F}PM")  // under the "Yesterday" header
        #expect(Dates.short(ts(15), now: now, locale: en) == "Thu")
        #expect(Dates.short(ts(2), now: now, locale: en) == "Oct 2")
        #expect(Dates.short(ts(2, year: 2025), now: now, locale: en) == "10/2/2025")
        #expect(Dates.section(ts(18), now: now, locale: en) == "Today")
        #expect(Dates.section(ts(17), now: now, locale: en) == "Yesterday")
        #expect(Dates.section(ts(14), now: now, locale: en) == "This Week")
        #expect(Dates.section(ts(2), now: now, locale: en) == "October")
        #expect(Dates.section(ts(2, month: 8, year: 2025), now: now, locale: en) == "August 2025")
        let zh = Locale(identifier: "zh_CN")
        #expect(Dates.short(ts(2), now: now, locale: zh) == "10月2日")
    }

    @Test func sizesAndDays() {
        #expect(Sizes.human(512) == "512 B")
        #expect(Sizes.human(820 * 1024) == "820 KB")
        #expect(Sizes.human(1_258_291) == "1.2 MB")
        #expect(SyncDays.label(0) == "All")
        #expect(SyncDays.label(365) == "1 year")
        #expect(SyncDays.sentence(30) == "Keeping the last 30 days")
        #expect(SyncDays.sentence(0) == "Keeping all mail")
    }

    @Test func oklchConvertsKnownColours() {
        // oklch(1 0 0) is white and oklch(0 0 0) black.
        let white = NSColor(Palette.oklch(1, 0, 0)).usingColorSpace(.sRGB)!
        let black = NSColor(Palette.oklch(0, 0, 0)).usingColorSpace(.sRGB)!
        #expect(abs(white.redComponent - 1) < 0.01 && abs(white.blueComponent - 1) < 0.01)
        #expect(black.redComponent < 0.01 && black.greenComponent < 0.01)
    }
}
