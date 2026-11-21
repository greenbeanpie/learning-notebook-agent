import SwiftUI

struct MainView: View {
    @EnvironmentObject var state: AppState
    @State private var chatInput = ""

    private let canvasBg = Color(red: 0x13/255, green: 0x13/255, blue: 0x14/255)
    private let panelBg = Color(red: 0x1E/255, green: 0x1F/255, blue: 0x20/255)
    private let elevatedBg = Color(red: 0x28/255, green: 0x2A/255, blue: 0x2C/255)

    var body: some View {
        VStack(spacing: 10) {
            topBar
            GeometryReader { geo in
                HStack(spacing: 12) {
                    if !state.studioFullscreen {
                        sourcesPane
                            .frame(width: state.activeArtifact != nil ? 250 : 300)
                        chatPane
                            .frame(maxWidth: .infinity)
                    }
                    studioPane
                        .frame(
                            width: state.studioFullscreen
                                ? geo.size.width
                                : (state.activeArtifact != nil ? max(460, geo.size.width * 0.48) : 340)
                        )
                }
            }
            if !state.statusMessage.isEmpty {
                HStack {
                    Text(state.statusMessage)
                        .font(.caption)
                        .foregroundColor(Color(white: 0.78))
                    Spacer()
                    Button("关闭") { state.statusMessage = "" }
                        .buttonStyle(.plain)
                        .font(.caption)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(elevatedBg)
                .cornerRadius(10)
            }
        }
        .padding(14)
        .background(canvasBg)
        .preferredColorScheme(.dark)
        .sheet(isPresented: $state.showSettingsSheet) {
            ModelSettingsSheet().environmentObject(state)
        }
    }

    private var topBar: some View {
        HStack(spacing: 12) {
            Text("Local Notebook Agent")
                .font(.headline)
                .foregroundColor(.white)
            Picker("", selection: $state.activeNotebookId) {
                ForEach(state.notebooks) { nb in
                    Text(nb.title).tag(nb.id)
                }
            }
            .frame(width: 200)
            .onChange(of: state.activeNotebookId) { _, _ in
                Task { await state.refreshAll() }
            }
            Spacer()
            Button("新建笔记本") {
                Task {
                    if let res = try? await state.core.call(method: "notebook.create", params: ["title": "新笔记本"]),
                       let id = res["id"] as? String {
                        state.activeNotebookId = id
                        await state.refreshAll()
                    }
                }
            }
            .buttonStyle(.bordered)
            Button("模型协议配置") { state.showSettingsSheet = true }
                .buttonStyle(.bordered)
        }
    }

    private var sourcesPane: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("来源").font(.subheadline.bold())
            HStack(spacing: 8) {
                Menu("+ 添加来源") {
                    Button("本地文件 (PDF / Markdown / 文本)") { pickAndAddFile() }
                    Button("粘贴文本") {
                        Task {
                            _ = try? await state.core.call(method: "source.add_text", params: [
                                "notebook_id": state.activeNotebookId,
                                "title": "粘贴文本.txt",
                                "text": "示例来源正文。"
                            ])
                            await state.refreshAll()
                        }
                    }
                }
                .menuStyle(.borderlessButton)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(elevatedBg)
                .cornerRadius(16)

                Button("+ 创建笔记") {
                    Task {
                        _ = try? await state.core.call(method: "note.save", params: [
                            "notebook_id": state.activeNotebookId,
                            "title": "新笔记",
                            "body_md": ""
                        ])
                        await state.refreshAll()
                    }
                }
                .buttonStyle(.bordered)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(state.sources) { src in
                        HStack {
                            Image(systemName: "doc.text")
                                .foregroundColor(Color(red: 0x8A/255, green: 0xB4/255, blue: 0xF8/255))
                            Text(src.title).lineLimit(1).font(.callout)
                            Spacer()
                            Toggle("", isOn: Binding(
                                get: { src.isSelected },
                                set: { val in
                                    Task {
                                        _ = try? await state.core.call(method: "source.set_selected", params: ["source_id": src.id, "selected": val])
                                        await state.refreshAll()
                                    }
                                }
                            ))
                            .labelsHidden()
                        }
                        .padding(8)
                        .background(elevatedBg.opacity(0.6))
                        .cornerRadius(10)
                    }
                }
            }
        }
        .padding(14)
        .background(panelBg)
        .cornerRadius(16)
    }

    private var chatPane: some View {
        VStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                Text(state.summaryText)
                    .font(.callout)
                    .foregroundColor(Color(white: 0.78))
                    .lineLimit(5)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(elevatedBg)
            .cornerRadius(12)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    ForEach(state.messages) { msg in
                        ChatMessageRow(message: msg)
                    }
                }
            }

            HStack(spacing: 8) {
                TextField("提问或创作内容...", text: $chatInput)
                    .textFieldStyle(.plain)
                    .onSubmit { submitChat() }
                Button(action: submitChat) {
                    Image(systemName: "arrow.up")
                        .foregroundColor(.black)
                        .frame(width: 32, height: 32)
                        .background(Color(red: 0xA8/255, green: 0xC7/255, blue: 0xFA/255))
                        .clipShape(Circle())
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(elevatedBg)
            .cornerRadius(24)
        }
        .padding(16)
        .background(panelBg)
        .cornerRadius(16)
    }

    private var studioPane: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let art = state.activeArtifact {
                ArtifactViewerView(artifact: art)
                    .environmentObject(state)
            } else {
                Text("Studio").font(.subheadline.bold())
                let tiles: [(String, String, String)] = [
                    ("audio", "音频概览", "waveform"),
                    ("slides", "演示文稿", "rectangle.stack"),
                    ("mindmap", "思维导图", "point.3.connected.trianglepath.dotted"),
                    ("report", "报告", "doc.richtext"),
                    ("flashcards", "闪卡", "square.on.square"),
                    ("quiz", "测验", "checkmark.circle"),
                    ("infographic", "信息图", "chart.bar.xaxis"),
                ]
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                    ForEach(tiles, id: \.0) { kind, label, icon in
                        Button {
                            Task { await state.createStudio(kind: kind) }
                        } label: {
                            HStack(spacing: 8) {
                                Image(systemName: icon)
                                Text(label).font(.callout)
                                Spacer()
                            }
                            .padding(10)
                            .background(elevatedBg)
                            .cornerRadius(12)
                        }
                        .buttonStyle(.plain)
                    }
                }
                ScrollView {
                    VStack(spacing: 8) {
                        ForEach(state.artifacts) { art in
                            Button {
                                state.activeArtifact = art
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(art.title).font(.callout.weight(.medium))
                                        Text("\(art.artifactType) · \(art.status)")
                                            .font(.caption)
                                            .foregroundColor(.secondary)
                                    }
                                    Spacer()
                                }
                                .padding(10)
                                .background(elevatedBg)
                                .cornerRadius(12)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(panelBg)
        .cornerRadius(16)
    }

    private func submitChat() {
        let txt = chatInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !txt.isEmpty else { return }
        chatInput = ""
        Task { await state.sendChat(txt) }
    }

    private func pickAndAddFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                _ = try? await state.core.call(method: "source.add_files", params: [
                    "notebook_id": state.activeNotebookId,
                    "paths": [url.path]
                ])
                await state.refreshAll()
            }
        }
    }
}

struct ChatMessageRow: View {
    let message: ChatMessageItem
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(message.role == "user" ? "你" : "助手")
                .font(.caption)
                .foregroundColor(.secondary)

            if message.role == "assistant" && !message.thoughts.isEmpty {
                Button {
                    expanded.toggle()
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "circle")
                            .font(.system(size: 11, weight: .bold))
                        Text("Thoughts")
                            .font(.callout)
                        Image(systemName: expanded ? "chevron.up" : "chevron.down")
                            .font(.caption)
                    }
                    .foregroundColor(Color(white: 0.78))
                }
                .buttonStyle(.plain)

                if expanded {
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(message.thoughts) { step in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 8) {
                                    Image(systemName: step.isTool ? "doc.text.magnifyingglass" : "brain")
                                        .foregroundColor(Color(white: 0.78))
                                    Text(step.title)
                                        .font(.callout.weight(step.isTool ? .regular : .medium))
                                }
                                if !step.isTool && !step.detail.isEmpty {
                                    HStack(spacing: 10) {
                                        Rectangle()
                                            .fill(Color(white: 0.25))
                                            .frame(width: 1)
                                            .padding(.leading, 7)
                                        Text(step.detail)
                                            .font(.callout)
                                            .foregroundColor(Color(white: 0.62))
                                    }
                                }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }

            Text(message.content)
                .textSelection(.enabled)
                .font(.body)
        }
    }
}
