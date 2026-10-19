import SwiftUI
import LiteMailCore

struct MainView: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @State private var columns = NavigationSplitViewVisibility.all

    var body: some View {
        NavigationSplitView(columnVisibility: $columns) {
            SidebarView(model: model)
                .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 300)
        } content: {
            MessageListView(model: model)
                .navigationSplitViewColumnWidth(min: 320, ideal: 400, max: 560)
        } detail: {
            ReaderColumn(model: model)
        }
        .tint(Theme.accent)
        .searchable(text: $model.search, placement: .toolbar, prompt: Text("Search all mail"))
        .searchScopes($model.searchScope) {
            Text("All Mail").tag(SearchScope.everything)
            Text("This Mailbox").tag(SearchScope.current)
        }
        .sheet(isPresented: $model.presentingAddAccount) {
            AddAccountView(model: model)
        }
        .sheet(item: Binding(
            get: { model.sourceText.map { SourceText(text: $0) } },
            set: { model.sourceText = $0?.text }
        )) { src in
            SourceView(text: src.text)
        }
        .overlay(alignment: .bottom) { BannerView(model: model) }
        .onOpenURL { url in
            // LiteMail can be the default mail app: mailto: links open a compose window.
            if url.scheme == "mailto" {
                openWindow(value: ComposeRequest(kind: .mailto, accountId: model.accounts.first?.id, to: url.absoluteString))
            }
        }
        .onChange(of: model.automationSettings) { _, open in
            if open { openSettings(); model.automationSettings = false }
        }
        .onChange(of: model.automationCompose) { _, req in
            if let req { openWindow(value: req); model.automationCompose = nil }
        }
        .frame(minWidth: 960, minHeight: 600)
    }
}

private struct SourceText: Identifiable {
    let text: String
    var id: Int { text.hashValue }
}

struct SourceView: View {
    let text: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                Text(text)
                    .font(.system(size: 11.5, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
            .background(Theme.paper)
            Divider()
            HStack {
                Text("Message Source").font(Theme.serif(15, .medium))
                Spacer()
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                }
                Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 760, height: 560)
        .tint(Theme.accent)
    }
}

/// Transient error / info message at the bottom of the window.
struct BannerView: View {
    @Bindable var model: AppModel

    var body: some View {
        if let b = model.banner {
            HStack(spacing: 10) {
                Image(systemName: b.isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                    .foregroundStyle(b.isError ? Theme.flag : Theme.accent)
                Text(b.text).font(.callout).lineLimit(2)
                Button {
                    model.banner = nil
                } label: {
                    Image(systemName: "xmark").font(.caption.weight(.semibold))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: .capsule)
            .padding(.bottom, 18)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .id(b.id)
            .task(id: b.id) {
                try? await Task.sleep(for: .seconds(b.isError ? 6 : 3))
                if model.banner?.id == b.id { withAnimation { model.banner = nil } }
            }
        }
    }
}
