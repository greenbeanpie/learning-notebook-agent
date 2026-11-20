using System.Runtime.InteropServices;

namespace LocalNotebook.Interop;

internal static unsafe partial class LnaNative
{
    private const string DllName = "lna_ffi";

    [LibraryImport(DllName, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial int lna_open(
        string configJson,
        delegate* unmanaged[Cdecl]<byte*, nuint, nint, void> onEvent,
        nint user,
        out nint core,
        out byte* outError);

    [LibraryImport(DllName, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial void lna_call(
        nint core,
        string requestJson,
        delegate* unmanaged[Cdecl]<byte*, nuint, nint, void> onReply,
        nint user);

    [LibraryImport(DllName)]
    internal static partial void lna_close(nint core);

    [LibraryImport(DllName)]
    internal static partial void lna_string_free(byte* s);
}
