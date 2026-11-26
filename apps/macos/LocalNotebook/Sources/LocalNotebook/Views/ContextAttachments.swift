import AppKit
import SwiftUI
import UniformTypeIdentifiers

// "+ Add context": pictures, one-off reference files and web links attached to the next message
// of a text input. The logic lives in AttachmentModel.swift; this file is the SwiftUI side.

/// The "+" button with its menu (image / file / web link).
struct AttachMenuButton: View {
    @ObservedObject var store: AttachmentStore
    let en: Bool
    var tint: Color = .secondary

    @State private var askingLink = false
    @State private var linkText = ""

    var body: some View {
        Menu {
            Section(L10n.t("ctx.add", en: en)) {
                Button {
                    pick(imagesOnly: true)
                } label: {
                    Label(L10n.t("ctx.image", en: en), systemImage: "photo")
                }
                Button {
                    pick(imagesOnly: false)
                } label: {
                    Label(L10n.t("ctx.file", en: en), systemImage: "doc")
                }
                Button {
                    linkText = ""
                    askingLink = true
                } label: {
                    Label(L10n.t("ctx.link", en: en), systemImage: "link")
                }
            }
        } label: {
            Image(systemName: "plus.circle")
                .font(.system(size: 18))
                .foregroundColor(tint)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(L10n.t("ctx.add", en: en))
        .sheet(isPresented: $askingLink) {
            VStack(alignment: .leading, spacing: 14) {
                Text(L10n.t("ctx.link_title", en: en)).font(.headline)
                TextField("https://", text: $linkText)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { addLink() }
                if !store.notice.isEmpty {
                    Text(store.notice).font(.caption).foregroundColor(.secondary)
                }
                HStack {
                    Spacer()
                    Button(L10n.t("common.cancel", en: en)) { askingLink = false }
                    Button(L10n.t("ctx.link_add", en: en)) { addLink() }
                        .buttonStyle(.borderedProminent)
                }
            }
            .padding(20)
            .frame(width: 420)
        }
    }

    private func addLink() {
        if store.addLink(linkText, en: en) {
            askingLink = false
        }
    }

    private func pick(imagesOnly: Bool) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        if imagesOnly {
            panel.allowedContentTypes = [.image]
        }
        if panel.runModal() == .OK, !panel.urls.isEmpty {
            store.add(urls: panel.urls, en: en)
        }
    }
}

/// The attached items waiting to be sent, each with a remove button.
struct AttachChips: View {
    @ObservedObject var store: AttachmentStore
    let en: Bool

    var body: some View {
        if !store.items.isEmpty || !store.notice.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                if !store.items.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(store.items) { item in
                                chip(item)
                            }
                        }
                    }
                }
                if !store.notice.isEmpty {
                    Text(store.notice).font(.caption).foregroundColor(.secondary)
                }
            }
        }
    }

    private func chip(_ item: AttachedItem) -> some View {
        HStack(spacing: 6) {
            if item.kind == .image, let data = item.data, let img = NSImage(data: data) {
                Image(nsImage: img)
                    .resizable()
                    .scaledToFill()
                    .frame(width: 34, height: 34)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
            } else {
                Image(systemName: item.kind == .link ? "link" : "doc.text")
                    .foregroundColor(.secondary)
            }
            Text(item.kind == .file ? "\(item.name) · \(Attach.sizeText(item.size))" : item.name)
                .font(.caption)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: 200, alignment: .leading)
            Button {
                store.remove(item.id)
            } label: {
                Image(systemName: "xmark.circle.fill").foregroundColor(.secondary)
            }
            .buttonStyle(.plain)
            .help(L10n.t("ctx.remove", en: en))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(Color.primary.opacity(0.07))
        .clipShape(RoundedRectangle(cornerRadius: 9))
    }
}

extension View {
    /// Lets files dropped on the view be attached.
    func attachDrop(_ store: AttachmentStore, en: Bool) -> some View {
        attachPaste(store, en: en).onDrop(of: [UTType.fileURL], isTargeted: nil) { providers in
            var accepted = false
            for p in providers where p.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                accepted = true
                _ = p.loadObject(ofClass: URL.self) { url, _ in
                    guard let url else { return }
                    DispatchQueue.main.async { store.add(urls: [url], en: en) }
                }
            }
            return accepted
        }
    }
}

/// The attachments of a stored message: pictures as thumbnails, files and links as chips.
/// Clicking opens them; "Add to sources" imports a file or link into the notebook.
struct MessageAttachments: View {
    let attachments: [[String: Any]]
    let en: Bool
    var onPromote: ((String) -> Void)? = nil

    var body: some View {
        if !attachments.isEmpty {
            WrapFlowLayout(spacing: 8) {
                ForEach(Array(attachments.enumerated()), id: \.offset) { _, att in
                    item(att)
                }
            }
        }
    }

    private func item(_ att: [String: Any]) -> some View {
        let kind = att["kind"] as? String ?? "image"
        let name = att["name"] as? String ?? ""
        let path = att["path"] as? String ?? ""
        let id = att["id"] as? String ?? ""
        return HStack(spacing: 6) {
            Button {
                open(att)
            } label: {
                HStack(spacing: 6) {
                    if kind == "image", let img = NSImage(contentsOfFile: path) {
                        Image(nsImage: img)
                            .resizable()
                            .scaledToFit()
                            .frame(maxWidth: 200, maxHeight: 150)
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                    } else {
                        Image(systemName: kind == "link" ? "link" : "doc.text").foregroundColor(.secondary)
                        Text(label(att, kind: kind, name: name))
                            .font(.caption)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: 220, alignment: .leading)
                    }
                }
            }
            .buttonStyle(.plain)
            if kind != "image", let onPromote, !id.isEmpty {
                Button(L10n.t("ctx.promote", en: en)) { onPromote(id) }
                    .buttonStyle(.link)
                    .font(.caption)
            }
        }
        .padding(.horizontal, kind == "image" ? 0 : 8)
        .padding(.vertical, kind == "image" ? 0 : 5)
        .background(kind == "image" ? Color.clear : Color.primary.opacity(0.07))
        .clipShape(RoundedRectangle(cornerRadius: 9))
    }

    private func label(_ att: [String: Any], kind: String, name: String) -> String {
        if kind == "link", let url = att["url"] as? String { return Attach.linkLabel(url) }
        let bytes = att["bytes"] as? Int ?? 0
        return "\(name) · \(Attach.sizeText(bytes))"
    }

    private func open(_ att: [String: Any]) {
        if (att["kind"] as? String) == "link" {
            if let s = att["url"] as? String, let u = URL(string: s) { NSWorkspace.shared.open(u) }
        } else if let p = att["path"] as? String, !p.isEmpty {
            NSWorkspace.shared.open(URL(fileURLWithPath: p))
        }
    }
}
