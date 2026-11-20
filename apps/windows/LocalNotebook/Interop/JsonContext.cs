using System.Text.Json;
using System.Text.Json.Serialization;

namespace LocalNotebook.Interop;

public sealed class OpenConfigDto
{
    [JsonPropertyName("data_dir")]
    public string DataDir { get; set; } = string.Empty;

    [JsonPropertyName("allow_memory_secret_fallback")]
    public bool AllowMemorySecretFallback { get; set; }
}

public sealed class ApiRequestDto
{
    [JsonPropertyName("id")]
    public string Id { get; set; } = string.Empty;

    [JsonPropertyName("method")]
    public string Method { get; set; } = string.Empty;

    [JsonPropertyName("params")]
    public JsonElement Params { get; set; }
}

public sealed class ApiErrorDto
{
    [JsonPropertyName("code")]
    public string Code { get; set; } = string.Empty;

    [JsonPropertyName("message")]
    public string Message { get; set; } = string.Empty;

    [JsonPropertyName("retryable")]
    public bool Retryable { get; set; }
}

public sealed class ApiResponseDto
{
    [JsonPropertyName("id")]
    public string Id { get; set; } = string.Empty;

    [JsonPropertyName("result")]
    public JsonElement? Result { get; set; }

    [JsonPropertyName("error")]
    public ApiErrorDto? Error { get; set; }
}

public sealed class ApiEventDto
{
    [JsonPropertyName("seq")]
    public ulong Seq { get; set; }

    [JsonPropertyName("topic")]
    public string Topic { get; set; } = string.Empty;

    [JsonPropertyName("event")]
    public string Event { get; set; } = string.Empty;

    [JsonPropertyName("data")]
    public JsonElement Data { get; set; }
}

[JsonSerializable(typeof(OpenConfigDto))]
[JsonSerializable(typeof(ApiRequestDto))]
[JsonSerializable(typeof(ApiResponseDto))]
[JsonSerializable(typeof(ApiEventDto))]
[JsonSerializable(typeof(string))]
internal partial class LnaJsonContext : JsonSerializerContext
{
}
