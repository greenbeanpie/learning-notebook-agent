import AppKit
import SwiftUI

/// Native modal helpers (the Windows client uses ContentDialog for these).
enum Confirm {
    /// A yes/no question; `destructive` styles the primary button as a warning.
    @MainActor
    static func ask(title: String, message: String, primary: String, cancel: String, destructive: Bool = false) -> Bool {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = destructive ? .warning : .informational
        let ok = alert.addButton(withTitle: primary)
        if destructive { ok.hasDestructiveAction = true }
        alert.addButton(withTitle: cancel)
        return alert.runModal() == .alertFirstButtonReturn
    }

    /// A text prompt; nil when cancelled.
    @MainActor
    static func prompt(title: String, label: String, initial: String = "", primary: String, cancel: String) -> String? {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = label
        alert.addButton(withTitle: primary)
        alert.addButton(withTitle: cancel)
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
        field.stringValue = initial
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        return field.stringValue
    }
}

enum PrivacyAlert {
    static func lines(en: Bool) -> [String] {
        ["privacy.line_local", "privacy.line_send", "privacy.line_keys", "privacy.line_diag"].map { L10n.t($0, en: en) }
    }

    /// Asks the user to acknowledge that prompts leave this computer. True = continue.
    @MainActor
    static func confirm(en: Bool, title: String) -> Bool {
        Confirm.ask(
            title: title,
            message: lines(en: en).joined(separator: "\n\n"),
            primary: L10n.t("dlg.got_it_continue", en: en),
            cancel: L10n.t("common.cancel", en: en)
        )
    }
}

/// Which source the left pane previews, and where to jump.
struct SourcePreviewRequest: Equatable {
    var sourceId: String
    var page: Int? = nil
    var focusText: String? = nil
    var title: String? = nil
}

extension AppState {
    /// After the core is ready: no model configured → a hint; first run → quick start.
    func afterStartup() async {
        let profiles = (try? await core.call(method: "profile.list"))?["profiles"] as? [[String: Any]] ?? []
        if !profiles.contains(where: { $0["is_active"] as? Bool == true }) {
            statusMessage = "\(t("settings.no_model_title"))\(L10n.t("common.colon", en: isEnglish))\(t("settings.no_model_msg"))"
            return
        }
        await showOnboardingIfFirstRun()
    }

    func showOnboardingIfFirstRun() async {
        if let r = try? await core.call(method: "settings.get", params: ["key": "onboarding.done"]), r["value"] as? Bool == true { return }
        _ = try? await core.call(method: "settings.set", params: ["key": "onboarding.done", "value": true])
        let en = isEnglish
        let body = ["onboard.intro", "onboard.step1", "onboard.step2", "onboard.step3"].map { L10n.t($0, en: en) }
        if Confirm.ask(title: L10n.t("dlg.quick_start", en: en), message: body.joined(separator: "\n\n"),
                       primary: L10n.t("dlg.open_settings", en: en), cancel: L10n.t("dlg.later", en: en)) {
            showSettingsSheet = true
        }
    }
}

extension AppState {
    /// Shows a source in the left pane (page / excerpt highlighted when given).
    func openSourcePreview(_ req: SourcePreviewRequest) {
        studioFullscreen = false
        previewSource = req
    }

    func closeSourcePreview() {
        previewSource = nil
        sourceFullscreen = false
    }
}
