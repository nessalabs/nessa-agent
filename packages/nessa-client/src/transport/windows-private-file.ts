/** Windows-only Node adapter. Built-in Windows PowerShell hosts a small Win32
 * bridge; filenames and secrets travel over stdin, never command arguments.
 * Validation and I/O use the same handle. No process-global mutable state. */
export async function windowsPrivateFile(
  operation: "read" | "reserve" | "write",
  path: string,
  value?: string,
): Promise<string> {
  const module = "node:child_process"
  const { spawn } = (await import(
    /* @vite-ignore */ module
  )) as typeof import("node:child_process")
  const root = process.env.SystemRoot
  if (!root || !/^[A-Za-z]:\\/.test(root))
    throw new Error("Windows system directory unavailable")
  return new Promise((resolve, reject) => {
    const child = spawn(
      `${root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe`,
      [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        Buffer.from(script, "utf16le").toString("base64"),
      ],
      { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] },
    )
    let output = ""
    const fail = () =>
      reject(new Error("Windows private credential file unavailable or unsafe"))
    const timer = setTimeout(() => {
      child.kill()
      fail()
    }, 15000)
    child.stdout.setEncoding("utf8")
    child.stdout.on("data", (chunk: string) => {
      output += chunk
      if (output.length > 32768) child.kill()
    })
    // PowerShell diagnostics can contain input; deliberately discard them.
    child.stderr.resume()
    child.on("error", () => {
      clearTimeout(timer)
      fail()
    })
    child.stdin.on("error", fail)
    child.on("close", (code) => {
      clearTimeout(timer)
      if (code !== 0 || output.length > 32768) {
        fail()
        return
      }
      try {
        resolve(Buffer.from(output.trim(), "base64").toString("utf8"))
      } catch {
        fail()
      }
    })
    child.stdin.end(JSON.stringify({ operation, path, value }))
  })
}

const script = String.raw`
$ErrorActionPreference = 'Stop'
try {
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;
public static class NessaPrivateFile {
  [StructLayout(LayoutKind.Sequential)] struct Attributes { public int Length; public IntPtr Descriptor; public int Inherit; }
  [StructLayout(LayoutKind.Sequential)] struct Info { public uint Attributes; public System.Runtime.InteropServices.ComTypes.FILETIME Creation, Access, Write; public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow; }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)] static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr attributes, uint disposition, uint flags, IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Info info);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)] static extern bool GetVolumeInformationByHandleW(SafeFileHandle handle, IntPtr name, uint length, IntPtr serial, IntPtr max, out uint flags, IntPtr fs, uint fsLength);
  [DllImport("advapi32.dll")] static extern uint GetSecurityInfo(SafeFileHandle handle, int type, uint flags, out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
  [DllImport("advapi32.dll")] static extern uint GetSecurityDescriptorLength(IntPtr descriptor);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr value);
  static void Require(bool condition) { if (!condition) throw new IOException("Unsafe private file"); }
  static SafeFileHandle Open(string path, uint access, uint disposition, IntPtr attributes) {
    var handle = CreateFileW(path, access, 7, attributes, disposition, 0x02200000, IntPtr.Zero);
    if (handle.IsInvalid) { handle.Dispose(); throw new IOException("Cannot open private file"); }
    return handle;
  }
  static Info Inspect(SafeFileHandle handle) {
    Info info; Require(GetFileInformationByHandle(handle, out info));
    Require((info.Attributes & 0x400) == 0); return info;
  }
  static void Verify(SafeFileHandle handle, SecurityIdentifier user) {
    Info info = Inspect(handle); Require((info.Attributes & 0x10) == 0 && info.Links == 1);
    uint flags; Require(GetVolumeInformationByHandleW(handle, IntPtr.Zero, 0, IntPtr.Zero, IntPtr.Zero, out flags, IntPtr.Zero, 0)); Require((flags & 8) != 0);
    IntPtr owner, group, dacl, sacl, sd;
    Require(GetSecurityInfo(handle, 1, 5, out owner, out group, out dacl, out sacl, out sd) == 0);
    try {
      byte[] bytes = new byte[GetSecurityDescriptorLength(sd)]; Marshal.Copy(sd, bytes, 0, bytes.Length);
      var descriptor = new RawSecurityDescriptor(bytes, 0);
      Require(user.Equals(descriptor.Owner) && descriptor.DiscretionaryAcl != null && (descriptor.ControlFlags & ControlFlags.DiscretionaryAclProtected) != 0);
      bool hasUser = false;
      foreach (GenericAce entry in descriptor.DiscretionaryAcl) {
        var ace = entry as CommonAce;
        Require(ace != null && !ace.IsCallback && ace.AceQualifier == AceQualifier.AccessAllowed && (ace.AceFlags & (AceFlags.Inherited | AceFlags.InheritOnly)) == 0);
        bool current = user.Equals(ace.SecurityIdentifier); hasUser |= current;
        Require(current || ace.SecurityIdentifier.IsWellKnown(WellKnownSidType.LocalSystemSid));
      }
      Require(hasUser);
    } finally { LocalFree(sd); }
  }
  public static string Run(string operation, string path, string value) {
    Require(Path.IsPathRooted(path) && path.Length > 3 && path[1] == ':' && path.Substring(2).IndexOf(':') < 0 && path.IndexOf('\0') < 0);
    path = Path.GetFullPath(path);
    for (string parent = Path.GetDirectoryName(path); parent != null && parent != Path.GetPathRoot(parent); parent = Path.GetDirectoryName(parent)) {
      using (var directory = Open(parent, 0x20080, 3, IntPtr.Zero)) { Inspect(directory); }
    }
    var user = WindowsIdentity.GetCurrent().User;
    Require(operation == "read" || operation == "write" || operation == "reserve");
    IntPtr sd = IntPtr.Zero, attributes = IntPtr.Zero;
    try {
      if (operation == "reserve") {
        var descriptor = new RawSecurityDescriptor("O:" + user.Value + "D:P(A;;FA;;;" + user.Value + ")(A;;FA;;;SY)");
        var bytes = new byte[descriptor.BinaryLength]; descriptor.GetBinaryForm(bytes, 0);
        sd = Marshal.AllocHGlobal(bytes.Length); Marshal.Copy(bytes, 0, sd, bytes.Length);
        var attr = new Attributes { Length = Marshal.SizeOf(typeof(Attributes)), Descriptor = sd };
        attributes = Marshal.AllocHGlobal(attr.Length); Marshal.StructureToPtr(attr, attributes, false);
      }
      using (var handle = Open(path, operation == "read" ? 0x80000000u : 0xC0000000u, operation == "reserve" ? 1u : 3u, attributes)) {
        Verify(handle, user);
        if (operation == "reserve") return "";
        using (var stream = new FileStream(handle, operation == "read" ? FileAccess.Read : FileAccess.ReadWrite)) {
          if (operation == "write") {
            Require(stream.Length == 0); var bytes = System.Text.Encoding.UTF8.GetBytes(value); Require(bytes.Length <= 16385);
            stream.Write(bytes, 0, bytes.Length); stream.Flush(true); return "";
          }
          Require(stream.Length > 0 && stream.Length <= 16385);
          var buffer = new byte[16386]; int count = 0, read;
          while (count < buffer.Length && (read = stream.Read(buffer, count, buffer.Length - count)) > 0) count += read;
          Require(count <= 16385); return Convert.ToBase64String(buffer, 0, count);
        }
      }
    } finally { if (attributes != IntPtr.Zero) Marshal.FreeHGlobal(attributes); if (sd != IntPtr.Zero) Marshal.FreeHGlobal(sd); }
  }
}
'@
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$result = [NessaPrivateFile]::Run($request.operation, $request.path, $request.value)
[Console]::Out.Write($result)
} catch { exit 1 }
`
