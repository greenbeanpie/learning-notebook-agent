using LocalNotebook.Interop;
using System.Text.Json;

namespace LocalNotebook.Services;

/// <summary>Thin helpers around the core: JSON built without reflection (NativeAOT safe) and typed accessors.</summary>
public static class Api
{
    public static JsonElement Args(params (string Key, object? Value)[] kv)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms))
        {
            w.WriteStartObject();
            foreach (var (k, v) in kv)
            {
                w.WritePropertyName(k);
                WriteValue(w, v, k);
            }
            w.WriteEndObject();
        }
        using var doc = JsonDocument.Parse(ms.ToArray());
        return doc.RootElement.Clone();
    }

    private static void WriteValue(Utf8JsonWriter w, object? v, string key)
    {
        switch (v)
        {
            case null: w.WriteNullValue(); break;
            case string s: w.WriteStringValue(s); break;
            case bool b: w.WriteBooleanValue(b); break;
            case int i: w.WriteNumberValue(i); break;
            case long l: w.WriteNumberValue(l); break;
            case double d: w.WriteNumberValue(d); break;
            case string[] arr:
                w.WriteStartArray();
                foreach (var a in arr) w.WriteStringValue(a);
                w.WriteEndArray();
                break;
            case int[] ints:
                w.WriteStartArray();
                foreach (var a in ints) w.WriteNumberValue(a);
                w.WriteEndArray();
                break;
            case JsonElement je: je.WriteTo(w); break;
            default: throw new ArgumentException($"unsupported arg type for {key}");
        }
    }

    public static async Task<(JsonElement? Result, ApiErrorDto? Error)> CallAsync(string method, JsonElement args)
    {
        var resp = await App.Core.CallAsync(method, args);
        return resp.Error is { } err ? (null, err) : (resp.Result ?? Args(), null);
    }

    public static string Str(JsonElement e, string name) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
            ? v.GetString() ?? "" : "";

    public static bool Bool(JsonElement e, string name) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.True;

    public static long Int(JsonElement e, string name, long fallback = 0) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetInt64(out var n)
            ? n : fallback;

    public static double Num(JsonElement e, string name, double fallback = 0) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetDouble(out var n)
            ? n : fallback;

    public static JsonElement Obj(JsonElement e, string name) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) ? v : default;

    public static IEnumerable<JsonElement> Arr(JsonElement e, string name)
    {
        if (e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Array)
        {
            foreach (var item in v.EnumerateArray()) yield return item;
        }
    }

    /// <summary>User-facing advice for the error codes a person can act on.</summary>
    public static string Advice(string code) => code switch
    {
        "MODEL_AUTH_FAILED" => "API Key 无效或没有权限，请在“模型协议配置”中检查密钥。",
        "MODEL_RATE_LIMITED" => "模型服务限流了，请稍后重试。",
        "MODEL_OVERLOADED" => "模型服务暂时不可用，请稍后重试。",
        "MODEL_NOT_FOUND" => "找不到该模型或接口地址，请检查模型 ID 与 Base URL。",
        "MODEL_REFUSED" => "模型拒绝回答这个问题。可以换一种问法再试。",
        "MODEL_STREAM_INTERRUPTED" => "回答中途中断，已保留已生成的内容；可点击“重新生成”。",
        "MODEL_CONTEXT_EXCEEDED" => "内容超出模型上下文，请减少勾选的来源或缩短问题。",
        "MODEL_NOT_CONFIGURED" => "请先在“模型协议配置”中配置模型。",
        "NO_SOURCE_SELECTED" => "请在左侧勾选至少一个已就绪的来源。",
        "FETCH_FAILED" => "无法连接服务，请检查网络或接口地址。",
        "SOURCE_NEEDS_OCR" => "这个 PDF 是扫描件，目前不支持识别图片中的文字。",
        "SOURCE_ENCRYPTED" => "这个 PDF 有密码保护，请先解除保护再导入。",
        "SOURCE_UNSUPPORTED" => "无法解析这个文件。",
        "SOURCE_TOO_LARGE" => "文件或文本超出大小限制。",
        "SOURCE_LIMIT_REACHED" => "单个笔记本最多 300 个来源。",
        "URL_BLOCKED" => "出于安全原因，不能导入本机或内网地址。",
        "CAPABILITY_MISSING" => "缺少所需组件，详见提示。",
        "OUTPUT_INVALID" => "模型输出不符合要求，请重试。",
        _ => "",
    };
}
