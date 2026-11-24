import SwiftUI
import AppKit

/// New-notebook dialog: name, a note that details are AI-generated, and optional source files.
struct CreateNotebookSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) private var dismiss

    let palette: AppThemePalette
    let preferredScheme: ColorScheme?

    @State private var name = ""
    @State private var files: [URL] = []
    @FocusState private var nameFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(state.t("nb.new"))
                .font(.headline)

            VStack(alignment: .leading, spacing: 6) {
                Text(state.t("nb.dlg_name")).font(.subheadline)
                TextField(state.t("nb.dlg_name_ph"), text: $name)
                    .textFieldStyle(.roundedBorder)
                    .focused($nameFocused)
                    .onSubmit { create() }
            }

            Text(state.t("nb.dlg_ai_hint"))
                .font(.caption)
                .foregroundColor(palette.textMuted)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 6) {
                Text(state.t("nb.dlg_sources")).font(.subheadline)
                Button(state.t("nb.dlg_add_files")) { pickFiles() }
                ForEach(files, id: \.self) { url in
                    HStack {
                        Text(url.lastPathComponent).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Button {
                            files.removeAll { $0 == url }
                        } label: { Image(systemName: "xmark") }
                        .buttonStyle(.borderless)
                    }
                }
            }

            HStack {
                Spacer()
                Button(state.t("common.cancel")) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(state.t("common.create")) { create() }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 460)
        .background(palette.panelBg)
        .preferredColorScheme(preferredScheme)
        .onAppear { nameFocused = true }
    }

    private func pickFiles() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        if panel.runModal() == .OK {
            for u in panel.urls where !files.contains(u) { files.append(u) }
        }
    }

    private func create() {
        let title = name
        guard !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let paths = files.map(\.path)
        dismiss()
        Task { await state.createNotebook(title: title, filePaths: paths) }
    }
}
