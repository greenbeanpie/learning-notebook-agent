import Foundation
#if canImport(AppKit)
import AppKit
#endif
#if canImport(Combine)
import Combine
#endif

/// What is attached to the next message: a picture, a one-off reference file (sent as is, the
/// agent reads it on demand) or a web link.
enum AttachKind: String {
    case image
    case file
    case link
}

struct AttachedItem: Identifiable, Equatable {
    let id = UUID()
    let kind: AttachKind
    let name: String
    var data: Data? = nil
    var url: String? = nil
    var size: Int = 0
}

enum Attach {
    /// Pictures, files and links per message (same limit as the core).
    static let maxItems = 6
    static let maxFileBytes = 200 * 1024 * 1024
    static let maxImageBytes = 40 * 1024 * 1024
    static let imageExts: Set<String> = ["png", "jpg", "jpeg", "webp", "gif", "bmp"]

    static func isImageName(_ name: String) -> Bool {
        let ext = (name as NSString).pathExtension.lowercased()
        return imageExts.contains(ext)
    }

    static func sizeText(_ bytes: Int) -> String {
        if bytes >= 1024 * 1024 {
            return String(format: "%.1f MB", Double(bytes) / 1024.0 / 1024.0)
        }
        return "\(max(bytes / 1024, 1)) KB"
    }

    static func isValidLink(_ s: String) -> Bool {
        let u = s.trimmingCharacters(in: .whitespacesAndNewlines)
        guard u.hasPrefix("http://") || u.hasPrefix("https://"), u.count > 8 else { return false }
        return u.rangeOfCharacter(from: .whitespacesAndNewlines) == nil
    }

    /// Host and path of a link, without the scheme.
    static func linkLabel(_ url: String) -> String {
        var rest = url
        if let r = url.range(of: "://") { rest = String(url[r.upperBound...]) }
        while rest.hasSuffix("/") { rest.removeLast() }
        return rest
    }

    /// Replaces `{0}`, `{1}`… of a localized string.
    static func format(_ template: String, _ args: [String]) -> String {
        var s = template
        for (i, a) in args.enumerated() {
            s = s.replacingOccurrences(of: "{\(i)}", with: a)
        }
        return s
    }
}

/// The attachments of one text input (chat, Studio request, outline revision, lecture question).
final class AttachmentStore: ObservableObject {
    @Published var items: [AttachedItem] = []
    /// A short message for the user about the last change (a skipped file, the limit…).
    @Published var notice: String = ""

    var isEmpty: Bool { items.isEmpty }

    func clear() {
        items.removeAll()
        notice = ""
    }

    func remove(_ id: UUID) {
        items.removeAll { $0.id == id }
    }

    func restore(_ saved: [AttachedItem]) {
        items = saved
        notice = ""
    }

    /// Adds files from disk; pictures become pictures, everything else a reference file.
    func add(urls: [URL], en: Bool) {
        var warn = ""
        for url in urls {
            if items.count >= Attach.maxItems {
                warn = Attach.format(L10n.t("ctx.too_many", en: en), [String(Attach.maxItems)])
                break
            }
            let name = url.lastPathComponent
            let image = Attach.isImageName(name)
            let limit = image ? Attach.maxImageBytes : Attach.maxFileBytes
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
            if size == 0 || size > limit {
                warn = Attach.format(L10n.t("ctx.too_large", en: en), [name, String(limit / 1024 / 1024)])
                continue
            }
            do {
                let bytes = try Data(contentsOf: url)
                items.append(AttachedItem(kind: image ? .image : .file, name: name, data: bytes, size: bytes.count))
            } catch {
                warn = Attach.format(L10n.t("ctx.read_failed", en: en), [name, error.localizedDescription])
            }
        }
        notice = warn
    }

#if canImport(AppKit)
    /// Attaches what a paste would bring in: copied files (Finder) or a picture (screenshot, "Copy Image").
    /// Returns false when the pasteboard holds plain text, which the text field should paste itself.
    @discardableResult
    func addFromPasteboard(_ pb: NSPasteboard, en: Bool) -> Bool {
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            add(urls: urls, en: en)
            return true
        }
        // A picture, unless the pasteboard also carries real text (copying spreadsheet cells puts both on it).
        let text = pb.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let textIsReal = !text.isEmpty && !Attach.isValidLink(text) && (text.contains(" ") || text.contains("\n"))
        guard !textIsReal, let png = Self.pngData(from: pb) else { return false }
        guard items.count < Attach.maxItems else {
            notice = Attach.format(L10n.t("ctx.too_many", en: en), [String(Attach.maxItems)])
            return true
        }
        let stamp = DateFormatter()
        stamp.dateFormat = "yyyyMMdd-HHmmss"
        let name = "pasted-image-\(stamp.string(from: Date())).png"
        guard png.count <= Attach.maxImageBytes else {
            notice = Attach.format(L10n.t("ctx.too_large", en: en), [name, String(Attach.maxImageBytes / 1024 / 1024)])
            return true
        }
        items.append(AttachedItem(kind: .image, name: name, data: png, size: png.count))
        notice = ""
        return true
    }

    private static func pngData(from pb: NSPasteboard) -> Data? {
        if let png = pb.data(forType: .png), !png.isEmpty { return png }
        guard let tiff = pb.data(forType: .tiff) ?? pb.data(forType: NSPasteboard.PasteboardType("public.jpeg")),
              let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }
#endif

    /// Returns false (and sets a notice) when `text` is not a web address.
    @discardableResult
    func addLink(_ text: String, en: Bool) -> Bool {
        let u = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard Attach.isValidLink(u) else {
            notice = L10n.t("ctx.link_bad", en: en)
            return false
        }
        guard items.count < Attach.maxItems else {
            notice = Attach.format(L10n.t("ctx.too_many", en: en), [String(Attach.maxItems)])
            return false
        }
        items.append(AttachedItem(kind: .link, name: Attach.linkLabel(u), url: u))
        notice = ""
        return true
    }

    /// Puts the attachments into a request as the core expects them (`images`, `files`, `links`).
    func merge(into params: inout [String: Any]) {
        var images: [[String: Any]] = []
        var files: [[String: Any]] = []
        var links: [String] = []
        for it in items {
            switch it.kind {
            case .link:
                links.append(it.url ?? "")
            case .file:
                files.append(["name": it.name, "data_base64": (it.data ?? Data()).base64EncodedString()])
            case .image:
                images.append(["name": it.name, "data_base64": (it.data ?? Data()).base64EncodedString()])
            }
        }
        if !images.isEmpty { params["images"] = images }
        if !files.isEmpty { params["files"] = files }
        if !links.isEmpty { params["links"] = links }
    }
}
