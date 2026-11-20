using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Threading;

namespace LocalNotebookAgent.Core.Protocols;

public enum ModelProtocolType
{
    ChatCompletion,
    ResponsesApi,
    Anthropic,
    Gemini
}

public sealed record ModelProfile(
    string Id,
    string Name,
    ModelProtocolType Protocol,
    string BaseUrl,
    string ModelId,
    string ApiKey,
    bool EnableThinking = true,
    int MaxTokens = 4096
);

public sealed record ChatMessageItem(string Role, string Content);

public sealed record UnifiedRequest(
    string SystemPrompt,
    IReadOnlyList<ChatMessageItem> Messages,
    double Temperature = 0.3
);

public abstract record AgentStreamEvent
{
    public sealed record ThoughtDelta(string Text) : AgentStreamEvent;
    public sealed record TextDelta(string Text) : AgentStreamEvent;
    public sealed record UsageReported(int InputTokens, int OutputTokens) : AgentStreamEvent;
    public sealed record Completed() : AgentStreamEvent;
    public sealed record Error(string Message) : AgentStreamEvent;
}

public interface IModelProtocolAdapter
{
    ModelProtocolType Protocol { get; }
    IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        CancellationToken ct = default);
}

/// <summary>
/// 统一协议路由器：供 WinUI 3 / NativeAOT 后端直接调用
/// </summary>
public sealed class ModelProtocolRouter
{
    private readonly Dictionary<ModelProtocolType, IModelProtocolAdapter> _adapters;

    public ModelProtocolRouter(HttpClient httpClient)
    {
        _adapters = new Dictionary<ModelProtocolType, IModelProtocolAdapter>
        {
            [ModelProtocolType.ChatCompletion] = new ChatCompletionAdapter(httpClient),
            [ModelProtocolType.ResponsesApi] = new ResponsesApiAdapter(httpClient),
            [ModelProtocolType.Anthropic] = new AnthropicAdapter(httpClient),
            [ModelProtocolType.Gemini] = new GeminiAdapter(httpClient)
        };
    }

    public IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        CancellationToken ct = default)
    {
        return _adapters[profile.Protocol].StreamAsync(profile, request, ct);
    }
}

internal static class SseHelper
{
    public static async IAsyncEnumerable<(string? EventName, string Data)> ReadSseAsync(
        HttpResponseMessage response,
        [EnumeratorCancellation] CancellationToken ct)
    {
        await using var stream = await response.Content.ReadAsStreamAsync(ct);
        using var reader = new StreamReader(stream, Encoding.UTF8);

        string? currentEvent = null;
        while (!reader.EndOfStream && !ct.IsCancellationRequested)
        {
            var line = await reader.ReadLineAsync(ct);
            if (line is null) break;

            if (line.StartsWith("event:", StringComparison.OrdinalIgnoreCase))
            {
                currentEvent = line[6..].Trim();
            }
            else if (line.StartsWith("data:", StringComparison.OrdinalIgnoreCase))
            {
                var data = line[5..].Trim();
                if (!string.IsNullOrEmpty(data))
                {
                    yield return (currentEvent, data);
                }
            }
            else if (string.IsNullOrWhiteSpace(line))
            {
                currentEvent = null;
            }
        }
    }
}

public sealed class ChatCompletionAdapter(HttpClient http) : IModelProtocolAdapter
{
    public ModelProtocolType Protocol => ModelProtocolType.ChatCompletion;

    public async IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        [EnumeratorCancellation] CancellationToken ct = default)
    {
        var messages = new List<object> { new { role = "system", content = request.SystemPrompt } };
        foreach (var m in request.Messages)
            messages.Add(new { role = m.Role, content = m.Content });

        var payload = JsonSerializer.Serialize(new
        {
            model = profile.ModelId,
            stream = true,
            temperature = request.Temperature,
            messages
        });

        using var req = new HttpRequestMessage(HttpMethod.Post, $"{profile.BaseUrl.TrimEnd('/')}/chat/completions")
        {
            Content = new StringContent(payload, Encoding.UTF8, "application/json")
        };
        req.Headers.Authorization = new AuthenticationHeaderValue("Bearer", profile.ApiKey);

        using var resp = await http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        resp.EnsureSuccessStatusCode();

        await foreach (var (_, data) in SseHelper.ReadSseAsync(resp, ct))
        {
            if (data == "[DONE]")
            {
                yield return new AgentStreamEvent.Completed();
                yield break;
            }
            using var doc = JsonDocument.Parse(data);
            if (doc.RootElement.TryGetProperty("choices", out var choices) && choices.GetArrayLength() > 0)
            {
                var delta = choices[0].GetProperty("delta");
                if (delta.TryGetProperty("reasoning_content", out var rc) && rc.ValueKind == JsonValueKind.String)
                    yield return new AgentStreamEvent.ThoughtDelta(rc.GetString()!);
                if (delta.TryGetProperty("content", out var c) && c.ValueKind == JsonValueKind.String)
                    yield return new AgentStreamEvent.TextDelta(c.GetString()!);
            }
        }
    }
}

public sealed class ResponsesApiAdapter(HttpClient http) : IModelProtocolAdapter
{
    public ModelProtocolType Protocol => ModelProtocolType.ResponsesApi;

    public async IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        [EnumeratorCancellation] CancellationToken ct = default)
    {
        var input = new List<object>();
        foreach (var m in request.Messages)
            input.Add(new { role = m.Role, content = m.Content });

        var payload = JsonSerializer.Serialize(new
        {
            model = profile.ModelId,
            instructions = request.SystemPrompt,
            stream = true,
            reasoning = profile.EnableThinking ? new { effort = "medium", summary = "auto" } : null,
            input
        }, new JsonSerializerOptions { DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull });

        using var req = new HttpRequestMessage(HttpMethod.Post, $"{profile.BaseUrl.TrimEnd('/')}/responses")
        {
            Content = new StringContent(payload, Encoding.UTF8, "application/json")
        };
        req.Headers.Authorization = new AuthenticationHeaderValue("Bearer", profile.ApiKey);

        using var resp = await http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        resp.EnsureSuccessStatusCode();

        await foreach (var (evt, data) in SseHelper.ReadSseAsync(resp, ct))
        {
            using var doc = JsonDocument.Parse(data);
            if (evt == "response.reasoning_summary_text.delta" && doc.RootElement.TryGetProperty("delta", out var rDelta))
                yield return new AgentStreamEvent.ThoughtDelta(rDelta.GetString() ?? "");
            else if (evt == "response.output_text.delta" && doc.RootElement.TryGetProperty("delta", out var tDelta))
                yield return new AgentStreamEvent.TextDelta(tDelta.GetString() ?? "");
            else if (evt == "response.completed")
            {
                yield return new AgentStreamEvent.Completed();
                yield break;
            }
        }
    }
}

public sealed class AnthropicAdapter(HttpClient http) : IModelProtocolAdapter
{
    public ModelProtocolType Protocol => ModelProtocolType.Anthropic;

    public async IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        [EnumeratorCancellation] CancellationToken ct = default)
    {
        var messages = new List<object>();
        foreach (var m in request.Messages)
            messages.Add(new { role = m.Role, content = m.Content });

        var payload = JsonSerializer.Serialize(new
        {
            model = profile.ModelId,
            max_tokens = profile.MaxTokens,
            system = request.SystemPrompt,
            stream = true,
            thinking = profile.EnableThinking ? new { type = "enabled", budget_tokens = 2048 } : null,
            messages
        }, new JsonSerializerOptions { DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull });

        using var req = new HttpRequestMessage(HttpMethod.Post, $"{profile.BaseUrl.TrimEnd('/')}/messages")
        {
            Content = new StringContent(payload, Encoding.UTF8, "application/json")
        };
        req.Headers.Add("x-api-key", profile.ApiKey);
        req.Headers.Add("anthropic-version", "2023-06-01");

        using var resp = await http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        resp.EnsureSuccessStatusCode();

        await foreach (var (evt, data) in SseHelper.ReadSseAsync(resp, ct))
        {
            using var doc = JsonDocument.Parse(data);
            if (evt == "content_block_delta" && doc.RootElement.TryGetProperty("delta", out var delta))
            {
                var type = delta.GetProperty("type").GetString();
                if (type == "thinking_delta" && delta.TryGetProperty("thinking", out var th))
                    yield return new AgentStreamEvent.ThoughtDelta(th.GetString() ?? "");
                else if (type == "text_delta" && delta.TryGetProperty("text", out var tx))
                    yield return new AgentStreamEvent.TextDelta(tx.GetString() ?? "");
            }
            else if (evt == "message_stop")
            {
                yield return new AgentStreamEvent.Completed();
                yield break;
            }
        }
    }
}

public sealed class GeminiAdapter(HttpClient http) : IModelProtocolAdapter
{
    public ModelProtocolType Protocol => ModelProtocolType.Gemini;

    public async IAsyncEnumerable<AgentStreamEvent> StreamAsync(
        ModelProfile profile,
        UnifiedRequest request,
        [EnumeratorCancellation] CancellationToken ct = default)
    {
        var contents = new List<object>();
        foreach (var m in request.Messages)
        {
            contents.Add(new
            {
                role = m.Role == "assistant" ? "model" : "user",
                parts = new[] { new { text = m.Content } }
            });
        }

        var payload = JsonSerializer.Serialize(new
        {
            systemInstruction = new { parts = new[] { new { text = request.SystemPrompt } } },
            contents,
            generationConfig = new
            {
                temperature = request.Temperature,
                thinkingConfig = profile.EnableThinking ? new { includeThoughts = true } : null
            }
        }, new JsonSerializerOptions { DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull });

        var url = $"{profile.BaseUrl.TrimEnd('/')}/models/{profile.ModelId}:streamGenerateContent?alt=sse";
        using var req = new HttpRequestMessage(HttpMethod.Post, url)
        {
            Content = new StringContent(payload, Encoding.UTF8, "application/json")
        };
        req.Headers.Add("x-goog-api-key", profile.ApiKey);

        using var resp = await http.SendAsync(req, HttpCompletionOption.ResponseHeadersRead, ct);
        resp.EnsureSuccessStatusCode();

        await foreach (var (_, data) in SseHelper.ReadSseAsync(resp, ct))
        {
            using var doc = JsonDocument.Parse(data);
            if (!doc.RootElement.TryGetProperty("candidates", out var candidates) || candidates.GetArrayLength() == 0)
                continue;
            var parts = candidates[0].GetProperty("content").GetProperty("parts");
            foreach (var part in parts.EnumerateArray())
            {
                var isThought = part.TryGetProperty("thought", out var th) && th.GetBoolean();
                if (part.TryGetProperty("text", out var txt))
                {
                    var s = txt.GetString() ?? "";
                    yield return isThought
                        ? new AgentStreamEvent.ThoughtDelta(s)
                        : new AgentStreamEvent.TextDelta(s);
                }
            }
        }
        yield return new AgentStreamEvent.Completed();
    }
}
