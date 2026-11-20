using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace LocalNotebook.Interop;

public sealed unsafe class CoreClient : IDisposable
{
    private nint _core;
    private GCHandle _selfHandle;
    private readonly ConcurrentDictionary<string, TaskCompletionSource<ApiResponseDto>> _pending = new();

    public event Action<ApiEventDto>? EventReceived;

    public void Open(string dataDir)
    {
        _selfHandle = GCHandle.Alloc(this);
        var cfg = JsonSerializer.Serialize(new OpenConfigDto
        {
            DataDir = dataDir,
            AllowMemorySecretFallback = false
        }, LnaJsonContext.Default.OpenConfigDto);

        int rc = LnaNative.lna_open(cfg, &OnEventUnmanaged, GCHandle.ToIntPtr(_selfHandle), out _core, out byte* errPtr);
        if (rc != 0)
        {
            string msg = errPtr != null ? Marshal.PtrToStringUTF8((nint)errPtr) ?? "lna_open failed" : "lna_open failed";
            if (errPtr != null) LnaNative.lna_string_free(errPtr);
            throw new InvalidOperationException(msg);
        }
    }

    public bool IsOpen => _core != 0;

    public Task<ApiResponseDto> CallAsync(string method, JsonElement paramsElement)
    {
        // A call before lna_open finished (or after close) must fail cleanly instead of passing a null handle to the core.
        if (_core == 0)
        {
            return Task.FromResult(new ApiResponseDto
            {
                Id = string.Empty,
                Error = new ApiErrorDto { Code = "NOT_READY", Message = "核心尚未就绪，请稍候再试" }
            });
        }

        var id = Guid.NewGuid().ToString("N");
        var tcs = new TaskCompletionSource<ApiResponseDto>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;

        var reqJson = JsonSerializer.Serialize(new ApiRequestDto
        {
            Id = id,
            Method = method,
            Params = paramsElement
        }, LnaJsonContext.Default.ApiRequestDto);

        LnaNative.lna_call(_core, reqJson, &OnReplyUnmanaged, GCHandle.ToIntPtr(_selfHandle));
        return tcs.Task;
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnEventUnmanaged(byte* json, nuint len, nint user)
    {
        if (user == 0 || json == null) return;
        var handle = GCHandle.FromIntPtr(user);
        if (handle.Target is not CoreClient client) return;
        var text = Encoding.UTF8.GetString(json, (int)len);
        var ev = JsonSerializer.Deserialize(text, LnaJsonContext.Default.ApiEventDto);
        if (ev != null) client.EventReceived?.Invoke(ev);
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnReplyUnmanaged(byte* json, nuint len, nint user)
    {
        if (user == 0 || json == null) return;
        var handle = GCHandle.FromIntPtr(user);
        if (handle.Target is not CoreClient client) return;
        var text = Encoding.UTF8.GetString(json, (int)len);
        var resp = JsonSerializer.Deserialize(text, LnaJsonContext.Default.ApiResponseDto);
        if (resp != null && client._pending.TryRemove(resp.Id, out var tcs))
        {
            tcs.TrySetResult(resp);
        }
    }

    public void Dispose()
    {
        if (_core != 0)
        {
            LnaNative.lna_close(_core);
            _core = 0;
        }
        if (_selfHandle.IsAllocated) _selfHandle.Free();
    }
}
