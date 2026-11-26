import AppKit
import SwiftUI

/// Cmd+V in a text input that has attachments: pictures and files on the pasteboard are attached instead of
/// being pasted as text (Finder puts the file path on the pasteboard next to the file, which is what a text
/// field would paste otherwise). Plain text still pastes normally.
@MainActor
final class PasteRouter {
    static let shared = PasteRouter()

    private struct Target {
        weak var view: NSView?
        let store: AttachmentStore
        let en: Bool
    }

    private var targets: [UUID: Target] = [:]
    private var monitor: Any?

    func register(id: UUID, view: NSView, store: AttachmentStore, en: Bool) {
        targets[id] = Target(view: view, store: store, en: en)
        if monitor == nil {
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self else { return event }
                return MainActor.assumeIsolated { self.handle(event) }
            }
        }
    }

    func unregister(id: UUID) {
        targets.removeValue(forKey: id)
    }

    private func handle(_ event: NSEvent) -> NSEvent? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard flags == .command, event.charactersIgnoringModifiers?.lowercased() == "v",
              let window = event.window ?? NSApp.keyWindow,
              let responder = window.firstResponder as? NSView else { return event }
        // Only while a text input is being edited.
        guard responder is NSTextView || responder is NSTextField else { return event }
        let center = responder.convert(CGPoint(x: responder.bounds.midX, y: responder.bounds.midY), to: nil)
        // The smallest registered container that holds the focused input is the one it belongs to.
        let match = targets.values
            .compactMap { t -> (Target, CGFloat)? in
                guard let v = t.view, v.window === window else { return nil }
                let r = v.convert(v.bounds, to: nil)
                guard r.contains(center) else { return nil }
                return (t, r.width * r.height)
            }
            .min { $0.1 < $1.1 }?.0
        guard let target = match else { return event }
        return target.store.addFromPasteboard(.general, en: target.en) ? nil : event
    }
}

private struct PasteAnchor: NSViewRepresentable {
    let id: UUID
    let store: AttachmentStore
    let en: Bool

    func makeNSView(context: Context) -> NSView { NSView() }

    func updateNSView(_ view: NSView, context: Context) {
        DispatchQueue.main.async {
            PasteRouter.shared.register(id: id, view: view, store: store, en: en)
        }
    }

    static func dismantleNSView(_ view: NSView, coordinator: ()) {
        // The id is not reachable here; stale entries are harmless because their view is gone (weak) and
        // they are skipped when `view.window` is nil.
    }
}

extension View {
    /// Registers this view as a paste target for `store` (see `PasteRouter`).
    func attachPaste(_ store: AttachmentStore, en: Bool) -> some View {
        modifier(PasteAttachModifier(store: store, en: en))
    }
}

private struct PasteAttachModifier: ViewModifier {
    let store: AttachmentStore
    let en: Bool
    @State private var id = UUID()

    func body(content: Content) -> some View {
        content
            .background(PasteAnchor(id: id, store: store, en: en))
            .onDisappear { PasteRouter.shared.unregister(id: id) }
    }
}
