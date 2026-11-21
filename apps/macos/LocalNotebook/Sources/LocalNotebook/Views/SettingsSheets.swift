import SwiftUI

struct ModelSettingsSheet: View {
    @EnvironmentObject var state: AppState
    @Environment(\.dismiss) var dismiss

    @State private var proto = "chat_completion"
    @State private var baseUrl = ""
    @State private var modelId = ""
    @State private var apiKey = ""
    @State private var statusMsg = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("模型协议配置").font(.headline)
            Picker("协议", selection: $proto) {
                Text("Chat Completions").tag("chat_completion")
                Text("Responses API").tag("responses")
                Text("Anthropic Messages").tag("anthropic")
                Text("Gemini GenerateContent").tag("gemini")
            }
            TextField("接口地址 (Base URL)", text: $baseUrl)
            TextField("模型 ID", text: $modelId)
            SecureField("API Key（存入 macOS Keychain）", text: $apiKey)
            if !statusMsg.isEmpty {
                Text(statusMsg).font(.caption).foregroundColor(.secondary)
            }
            HStack {
                Spacer()
                Button("关闭") { dismiss() }
                Button("保存并启用") {
                    Task {
                        do {
                            let prof = try await state.core.call(method: "profile.upsert", params: [
                                "display_name": "\(proto) / \(modelId)",
                                "protocol": proto,
                                "base_url": baseUrl,
                                "model_id": modelId,
                                "activate": true
                            ])
                            if let kref = prof["keychain_ref"] as? String, !apiKey.isEmpty {
                                _ = try await state.core.call(method: "secret.set", params: ["keychain_ref": kref, "secret": apiKey])
                            }
                            statusMsg = "已保存并启用。"
                            dismiss()
                        } catch {
                            statusMsg = "保存失败: \(error.localizedDescription)"
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .frame(width: 440)
    }
}
