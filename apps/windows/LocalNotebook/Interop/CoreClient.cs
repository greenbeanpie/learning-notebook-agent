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
    private volatile bool _disposed;
    private int _inflightCalls;
    private readonly ConcurrentDictionary<string, TaskCompletionSource<ApiResponseDto>> _pending = new();

    public event Action<ApiEventDto>? EventReceived;

    public void Open(string dataDir)
    {
        _disposed = false;
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
            if (_selfHandle.IsAllocated) _selfHandle.Free();
            throw new InvalidOperationException(msg);
        }
    }

    public bool IsOpen => _core != 0 && !_disposed;

    public Task<ApiResponseDto> CallAsync(string method, JsonElement paramsElement)
    {
        // A call before lna_open finished (or after close) must fail cleanly instead of passing a null handle to the core.
        if (_core == 0 || _disposed)
        {
            return Task.FromResult(new ApiResponseDto
            {
                Id = string.Empty,
                Error = new ApiErrorDto { Code = "NOT_READY", Message = Services.L10n.T("core.not_ready") }
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

        Interlocked.Increment(ref _inflightCalls);
        LnaNative.lna_call(_core, reqJson, &OnReplyUnmanaged, GCHandle.ToIntPtr(_selfHandle));
        return tcs.Task;
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnEventUnmanaged(byte* json, nuint len, nint user)
    {
        if (user == 0 || json == null) return;
        try
        {
            var handle = GCHandle.FromIntPtr(user);
            if (handle.Target is not CoreClient client || client._disposed) return;
            var text = Encoding.UTF8.GetString(json, (int)len);
            var ev = JsonSerializer.Deserialize(text, LnaJsonContext.Default.ApiEventDto);
            if (ev != null) client.EventReceived?.Invoke(ev);
        }
        catch
        {
            // Never let a managed exception escape an UnmanagedCallersOnly callback.
        }
    }

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnReplyUnmanaged(byte* json, nuint len, nint user)
    {
        if (user == 0 || json == null) return;
        try
        {
            var handle = GCHandle.FromIntPtr(user);
            if (handle.Target is not CoreClient client) return;
            var text = Encoding.UTF8.GetString(json, (int)len);
            var resp = JsonSerializer.Deserialize(text, LnaJsonContext.Default.ApiResponseDto);
            if (resp != null && client._pending.TryRemove(resp.Id, out var tcs))
            {
                tcs.TrySetResult(resp);
            }
            if (Interlocked.Decrement(ref client._inflightCalls) == 0 && client._disposed && client._selfHandle.IsAllocated)
            {
                client._selfHandle.Free();
            }
        }
        catch
        {
            // Never let a managed exception escape an UnmanagedCallersOnly callback.
        }
    }

    public void Dispose()
    {
        _disposed = true;
        if (_core != 0)
        {
            LnaNative.lna_close(_core);
            _core = 0;
        }
        foreach (var kv in _pending)
        {
            if (_pending.TryRemove(kv.Key, out var tcs))
            {
                tcs.TrySetResult(new ApiResponseDto
                {
                    Id = kv.Key,
                    Error = new ApiErrorDto { Code = "CANCELLED", Message = Services.L10n.T("core.closed") }
                });
            }
        }
        if (Volatile.Read(ref _inflightCalls) == 0 && _selfHandle.IsAllocated)
        {
            _selfHandle.Free();
        }
    }
}
