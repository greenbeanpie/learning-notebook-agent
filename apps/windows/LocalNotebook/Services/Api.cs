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

    /// <summary>Localized user-facing advice for error codes a user can act on.</summary>
    public static string Advice(string code) => L10n.Advice(code);
}
