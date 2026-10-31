using System.ComponentModel;
using System.Runtime.InteropServices;

namespace XeniaWorker;
internal static class Credentials
{
    const string Target = "XeniaWorker/worker-token";
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Credential { public uint Flags, Type; public string? TargetName, Comment; public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten; public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist, AttributeCount; public IntPtr Attributes; public string? TargetAlias, UserName; }
    [DllImport("advapi32.dll", EntryPoint="CredWriteW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CredWrite(ref Credential credential, uint flags);
    [DllImport("advapi32.dll", EntryPoint="CredReadW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CredRead(string target, uint type, uint flags, out IntPtr credential);
    [DllImport("advapi32.dll")] static extern void CredFree(IntPtr credential);
    public static string Load() {
        if (!CredRead(Target, 1, 0, out var ptr)) return "";
        try { var item = Marshal.PtrToStructure<Credential>(ptr); return Marshal.PtrToStringUni(item.CredentialBlob, (int)item.CredentialBlobSize / 2) ?? ""; } finally { CredFree(ptr); }
    }
    public static void Save(string value) {
        var blob = Marshal.StringToCoTaskMemUni(value);
        try { var item = new Credential { Type=1, TargetName=Target, UserName="worker", CredentialBlob=blob, CredentialBlobSize=(uint)(value.Length * 2), Persist=2 }; if (!CredWrite(ref item, 0)) throw new Win32Exception(Marshal.GetLastWin32Error()); }
        finally { Marshal.ZeroFreeCoTaskMemUnicode(blob); }
    }
}
