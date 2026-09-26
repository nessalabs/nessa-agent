[CmdletBinding()]
param(
    # The product model is a standard user's non-elevated token, and that is the
    # default. GitHub-hosted Windows runners are administrators with UAC off, so
    # the hosted leg declares Administrator: it proves the same identity and
    # cleanup agreement for an elevated caller and says so, and nothing more.
    # The declared context must match the caller token either way.
    [ValidateSet('StandardUser', 'Administrator')]
    [string] $CallerContext = 'StandardUser'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:TaskNotFoundHResult = -2147024894
$script:FileAllAccess = 0x001f01ff
$script:LocalSystemSid = 'S-1-5-18'
$script:CleanupFailures = [System.Collections.Generic.List[string]]::new()
$script:ObservedProcesses = [System.Collections.Generic.Dictionary[int, Microsoft.Win32.SafeHandles.SafeProcessHandle]]::new()

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public sealed class NessaTokenFacts
{
    public string Sid { get; set; }
    public bool Elevated { get; set; }
    public int ElevationType { get; set; }
    public string IntegritySid { get; set; }
    public long CreationTime { get; set; }
}

public static class NessaWindowsProofNative
{
    private const uint TOKEN_QUERY = 0x0008;
    private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    // WaitForSingleObject, which is how exit is observed, needs this right too.
    private const uint SYNCHRONIZE = 0x00100000;
    private const int TokenUser = 1;
    private const int TokenElevationType = 18;
    private const int TokenElevation = 20;
    private const int TokenIntegrityLevel = 25;
    private const int ERROR_BAD_LENGTH = 24;
    private const int ERROR_INSUFFICIENT_BUFFER = 122;
    private const uint FILE_READ_ATTRIBUTES = 0x0080;
    private const uint READ_CONTROL = 0x00020000;
    private const uint DELETE = 0x00010000;
    private const uint OPEN_EXISTING = 3;
    private const uint FILE_SHARE_READ_WRITE = 3;
    private const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    private const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    private const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;
    private const uint WAIT_OBJECT_0 = 0;
    private const uint WAIT_TIMEOUT = 258;
    private const uint WAIT_FAILED = 0xffffffff;

    [StructLayout(LayoutKind.Sequential)]
    private struct SECURITY_ATTRIBUTES
    {
        public int nLength;
        public IntPtr lpSecurityDescriptor;
        public int bInheritHandle;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct SID_AND_ATTRIBUTES
    {
        public IntPtr Sid;
        public uint Attributes;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BY_HANDLE_FILE_INFORMATION
    {
        public uint FileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct FILE_DISPOSITION_INFO
    {
        [MarshalAs(UnmanagedType.U1)]
        public bool DeleteFile;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateDirectoryW(string path, ref SECURITY_ATTRIBUTES attributes);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string path, uint access, uint share, IntPtr securityAttributes,
        uint creationDisposition, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle handle, out BY_HANDLE_FILE_INFORMATION information);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetFileInformationByHandle(
        SafeFileHandle handle, int informationClass,
        ref FILE_DISPOSITION_INFO information, uint size);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(
        string descriptor, uint revision, out IntPtr securityDescriptor, out uint size);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LocalFree(IntPtr memory);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern SafeProcessHandle OpenProcess(uint access, bool inherit, uint processId);

    [DllImport("kernel32.dll")]
    private static extern IntPtr GetCurrentProcess();

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint GetProcessId(SafeProcessHandle process);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(SafeProcessHandle handle, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetProcessTimes(
        SafeProcessHandle process,
        out System.Runtime.InteropServices.ComTypes.FILETIME creation,
        out System.Runtime.InteropServices.ComTypes.FILETIME exit,
        out System.Runtime.InteropServices.ComTypes.FILETIME kernel,
        out System.Runtime.InteropServices.ComTypes.FILETIME user);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(SafeProcessHandle process, uint access, out IntPtr token);

    [DllImport("advapi32.dll", EntryPoint = "OpenProcessToken", SetLastError = true)]
    private static extern bool OpenCurrentProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(
        IntPtr token, int informationClass, IntPtr information, int length, out int returnLength);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool ConvertSidToStringSidW(IntPtr sid, out IntPtr text);

    public static int CreatePrivateDirectory(string path, string descriptor)
    {
        IntPtr nativeDescriptor;
        uint size;
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(descriptor, 1, out nativeDescriptor, out size))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try
        {
            var attributes = new SECURITY_ATTRIBUTES {
                nLength = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)),
                lpSecurityDescriptor = nativeDescriptor,
                bInheritHandle = 0
            };
            if (CreateDirectoryW(path, ref attributes)) return 0;
            return Marshal.GetLastWin32Error();
        }
        finally
        {
            LocalFree(nativeDescriptor);
        }
    }

    public static SafeFileHandle OpenDirectory(string path)
    {
        var handle = CreateFileW(
            path, FILE_READ_ATTRIBUTES | READ_CONTROL | DELETE, FILE_SHARE_READ_WRITE,
            IntPtr.Zero, OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, IntPtr.Zero);
        if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        return handle;
    }

    private static BY_HANDLE_FILE_INFORMATION DirectoryInformation(SafeFileHandle handle)
    {
        BY_HANDLE_FILE_INFORMATION information;
        if (!GetFileInformationByHandle(handle, out information))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return information;
    }

    public static bool DirectoryIsReparsePoint(SafeFileHandle handle)
    {
        return (DirectoryInformation(handle).FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    }

    public static string DirectoryIdentity(SafeFileHandle handle)
    {
        var information = DirectoryInformation(handle);
        return String.Format(
            "{0:x8}:{1:x8}:{2:x8}", information.VolumeSerialNumber,
            information.FileIndexHigh, information.FileIndexLow);
    }

    public static void MarkDirectoryForDeletion(SafeFileHandle handle)
    {
        var disposition = new FILE_DISPOSITION_INFO { DeleteFile = true };
        if (!SetFileInformationByHandle(
            handle, 4, ref disposition,
            (uint)Marshal.SizeOf(typeof(FILE_DISPOSITION_INFO))))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }

    private static string SidText(IntPtr sid, string fact)
    {
        IntPtr text;
        if (!ConvertSidToStringSidW(sid, out text))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "ConvertSidToStringSidW failed for " + fact);
        try { return Marshal.PtrToStringUni(text); }
        finally { LocalFree(text); }
    }

    private static byte[] TokenInformation(IntPtr token, int informationClass, string fact)
    {
        int needed;
        bool sized = GetTokenInformation(token, informationClass, IntPtr.Zero, 0, out needed);
        int error = Marshal.GetLastWin32Error();
        if (needed <= 0 || (!sized && error != ERROR_BAD_LENGTH && error != ERROR_INSUFFICIENT_BUFFER))
            throw new Win32Exception(error, "GetTokenInformation size query failed for " + fact);
        var bytes = new byte[needed];
        var pinned = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        try
        {
            if (!GetTokenInformation(token, informationClass, pinned.AddrOfPinnedObject(), bytes.Length, out needed))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "GetTokenInformation fill failed for " + fact);
            return bytes;
        }
        finally { pinned.Free(); }
    }

    private static string TokenSid(IntPtr token, int informationClass, string fact)
    {
        var bytes = TokenInformation(token, informationClass, fact);
        var pinned = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        try
        {
            var value = (SID_AND_ATTRIBUTES)Marshal.PtrToStructure(
                pinned.AddrOfPinnedObject(), typeof(SID_AND_ATTRIBUTES));
            return SidText(value.Sid, fact);
        }
        finally { pinned.Free(); }
    }

    public static SafeProcessHandle OpenProcessForObservation(uint processId)
    {
        var process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, false, processId);
        if (process.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        uint openedId = GetProcessId(process);
        if (openedId == 0)
        {
            int error = Marshal.GetLastWin32Error();
            process.Dispose();
            throw new Win32Exception(error);
        }
        if (openedId != processId)
        {
            process.Dispose();
            throw new InvalidOperationException("opened process handle does not match the requested PID");
        }
        return process;
    }

    public static bool ProcessHasExited(SafeProcessHandle process)
    {
        uint result = WaitForSingleObject(process, 0);
        if (result == WAIT_OBJECT_0) return true;
        if (result == WAIT_TIMEOUT) return false;
        if (result == WAIT_FAILED) throw new Win32Exception(Marshal.GetLastWin32Error());
        throw new InvalidOperationException("unexpected process wait result " + result);
    }

    public static long ProcessCreationTime(SafeProcessHandle process)
    {
        System.Runtime.InteropServices.ComTypes.FILETIME creation, exit, kernel, user;
        if (!GetProcessTimes(process, out creation, out exit, out kernel, out user))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        return ((long)(uint)creation.dwHighDateTime << 32) | (uint)creation.dwLowDateTime;
    }

    public static NessaTokenFacts ReadTokenFacts(SafeProcessHandle process)
    {
        if (process == null || process.IsInvalid || process.IsClosed)
            throw new InvalidOperationException("process observation handle is not open");
        if (ProcessHasExited(process))
            throw new InvalidOperationException("process exited before its token could be observed");
        IntPtr token = IntPtr.Zero;
        try
        {
            if (!OpenProcessToken(process, TOKEN_QUERY, out token))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcessToken failed for retained process handle");
            return ReadTokenFacts(token, ProcessCreationTime(process));
        }
        finally
        {
            if (token != IntPtr.Zero) CloseHandle(token);
        }
    }

    public static NessaTokenFacts ReadCurrentProcessTokenFacts()
    {
        IntPtr token = IntPtr.Zero;
        try
        {
            if (!OpenCurrentProcessToken(GetCurrentProcess(), TOKEN_QUERY, out token))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcessToken failed for current-process pseudo-handle");
            return ReadTokenFacts(token, 0);
        }
        finally
        {
            if (token != IntPtr.Zero) CloseHandle(token);
        }
    }

    private static NessaTokenFacts ReadTokenFacts(IntPtr token, long creationTime)
    {
        return new NessaTokenFacts {
            Sid = TokenSid(token, TokenUser, "TokenUser"),
            Elevated = BitConverter.ToInt32(TokenInformation(token, TokenElevation, "TokenElevation"), 0) != 0,
            ElevationType = BitConverter.ToInt32(TokenInformation(token, TokenElevationType, "TokenElevationType"), 0),
            IntegritySid = TokenSid(token, TokenIntegrityLevel, "TokenIntegrityLevel"),
            CreationTime = creationTime
        };
    }
}
'@

function Get-NativeException {
    param([Parameter(Mandatory)] [System.Exception] $Exception)
    $native = $Exception
    while ($null -ne $native.InnerException) { $native = $native.InnerException }
    return $native
}

function Get-HResultHex {
    param([Parameter(Mandatory)] [System.Exception] $Exception)
    $native = Get-NativeException -Exception $Exception
    $bits = [System.BitConverter]::ToUInt32([System.BitConverter]::GetBytes([int]$native.HResult), 0)
    return ('0x{0:x8}' -f $bits)
}

function Test-NotFound {
    param([Parameter(Mandatory)] [System.Exception] $Exception)
    return (Get-NativeException -Exception $Exception).HResult -eq $script:TaskNotFoundHResult
}

function Assert-HResultClassification {
    $notFound = [System.Management.Automation.MethodInvocationException]::new(
        'wrapped not found', [System.Runtime.InteropServices.COMException]::new('not found', -2147024894))
    if (-not (Test-NotFound -Exception $notFound)) { throw 'wrapped task-not-found HRESULT was not recognized' }
    foreach ($lost in @(-2147417848, -2147417851, -2147023174, -2147023170)) {
        $wrapped = [System.Management.Automation.MethodInvocationException]::new(
            'wrapped lost reply', [System.Runtime.InteropServices.COMException]::new('lost reply', $lost))
        if ((Get-CallAcknowledgement -Exception $wrapped) -ne 'lost') { throw "wrapped lost-reply HRESULT $lost was not recognized" }
    }
    foreach ($rejected in @(-2147024891, -2147467259)) {
        $wrapped = [System.Management.Automation.MethodInvocationException]::new(
            'wrapped rejection', [System.Runtime.InteropServices.COMException]::new('rejected', $rejected))
        if ((Get-CallAcknowledgement -Exception $wrapped) -ne 'rejected') { throw "wrapped rejected HRESULT $rejected was not preserved" }
    }
}

function Get-CallAcknowledgement {
    param([Parameter(Mandatory)] [System.Exception] $Exception)
    $lostReplyHResults = @('0x80010108', '0x80010105', '0x800706ba', '0x800706be')
    if ($lostReplyHResults -contains (Get-HResultHex -Exception $Exception)) { return 'lost' }
    return 'rejected'
}

function Resolve-CreateOutcome {
    param(
        [Parameter(Mandatory)] [ValidateSet('success', 'lost', 'rejected')] [string] $Acknowledgement,
        [Parameter(Mandatory)] [ValidateSet('absent', 'matching', 'contradictory', 'unobservable')] [string] $Observation
    )
    $disposition = $null
    if ($Observation -eq 'absent') { $disposition = 'absent' }
    elseif ($Observation -eq 'unobservable') { $disposition = 'effect-uncertain' }
    if ($Acknowledgement -eq 'success') {
        if ($null -eq $disposition) { $disposition = "confirmed-created-$Observation" }
    }
    elseif ($Observation -eq 'matching') {
        $disposition = "observed-matching-after-$Acknowledgement-reply"
    }
    elseif ($null -eq $disposition) {
        $disposition = 'foreign-or-unresolved-contradictory'
    }
    return [pscustomobject]@{
        Disposition = $disposition
        AcceptanceEligible = $Acknowledgement -eq 'success' -and $Observation -eq 'matching'
        CleanupOwned = $disposition -like 'confirmed-created-*' -or $disposition -like 'observed-matching-*'
        Preserve = $disposition -eq 'foreign-or-unresolved-contradictory'
        Settled = $disposition -eq 'absent'
        Uncertain = $disposition -eq 'effect-uncertain'
    }
}

function Assert-LedgerMatrix {
    $expected = @{
        'success/absent' = 'absent|False|False|False|True|False'
        'success/matching' = 'confirmed-created-matching|True|True|False|False|False'
        'success/contradictory' = 'confirmed-created-contradictory|False|True|False|False|False'
        'success/unobservable' = 'effect-uncertain|False|False|False|False|True'
        'lost/absent' = 'absent|False|False|False|True|False'
        'lost/matching' = 'observed-matching-after-lost-reply|False|True|False|False|False'
        'lost/contradictory' = 'foreign-or-unresolved-contradictory|False|False|True|False|False'
        'lost/unobservable' = 'effect-uncertain|False|False|False|False|True'
        'rejected/absent' = 'absent|False|False|False|True|False'
        'rejected/matching' = 'observed-matching-after-rejected-reply|False|True|False|False|False'
        'rejected/contradictory' = 'foreign-or-unresolved-contradictory|False|False|True|False|False'
        'rejected/unobservable' = 'effect-uncertain|False|False|False|False|True'
    }
    foreach ($entry in $expected.GetEnumerator()) {
        $parts = $entry.Key.Split('/')
        $outcome = Resolve-CreateOutcome -Acknowledgement $parts[0] -Observation $parts[1]
        $actual = [string]::Join('|', @(
            $outcome.Disposition, $outcome.AcceptanceEligible, $outcome.CleanupOwned,
            $outcome.Preserve, $outcome.Settled, $outcome.Uncertain))
        if ($actual -ne $entry.Value) {
            throw "ledger matrix $($entry.Key) resolved to $actual, expected $($entry.Value)"
        }
    }
}

function Test-RunCleanupRequired {
    param(
        [bool] $TaskOwned,
        [bool] $RunAttempted,
        [ValidateSet('matching', 'contradictory', 'multiple', 'absent', 'unobservable')] [string] $Observation
    )
    return $TaskOwned -and $RunAttempted
}

function New-RunAttemptObservation {
    return [pscustomobject]@{
        Rejections = [System.Collections.Generic.List[string]]::new()
    }
}

function Update-RunAttemptObservation {
    param(
        [Parameter(Mandatory)] $Observation,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Instances,
        [Parameter(Mandatory)] [string] $ExpectedPath,
        [Parameter(Mandatory)] [string] $ExpectedActionId,
        [string] $ExpectedGuid = '',
        [int] $ExpectedPid = 0
    )
    if ($Instances.Count -gt 1) {
        $Observation.Rejections.Add("observed $($Instances.Count) instances for one IgnoreNew task")
    }
    foreach ($instance in $Instances) {
        if ([string]$instance.Path -ne $ExpectedPath) { $Observation.Rejections.Add("running task path contradicted $ExpectedPath") }
        if (-not [string]::IsNullOrWhiteSpace([string]$instance.CurrentAction) -and [string]$instance.CurrentAction -ne $ExpectedActionId) {
            $Observation.Rejections.Add("current action contradicted $ExpectedActionId")
        }
        if (-not [string]::IsNullOrWhiteSpace($ExpectedGuid) -and [string]$instance.InstanceGuid -ne $ExpectedGuid) {
            $Observation.Rejections.Add("instance GUID contradicted $ExpectedGuid")
        }
        if ($ExpectedPid -ne 0 -and [int]$instance.EnginePID -ne 0 -and [int]$instance.EnginePID -ne $ExpectedPid) {
            $Observation.Rejections.Add("EnginePID contradicted $ExpectedPid")
        }
    }
    return $Observation
}

function Assert-RunObservationAccepted {
    param([Parameter(Mandatory)] $Observation, [Parameter(Mandatory)] [string] $Attempt)
    if ($Observation.Rejections.Count -ne 0) {
        throw "$Attempt permanently rejected: $([string]::Join('; ', @($Observation.Rejections)))"
    }
}

function Test-StabilizedRunSnapshot {
    param([Parameter(Mandatory)] $Accepted, [Parameter(Mandatory)] $Snapshot)
    if ($Snapshot.InstanceCount -ne 1 -or $Snapshot.Path -ne $Accepted.PlannedTaskPath) { return $false }
    if ($Snapshot.CurrentAction -ne $Accepted.PlannedActionId -or $Snapshot.InstanceGuid -ne $Accepted.InstanceGuid) { return $false }
    if ($Snapshot.EnginePid -ne $Accepted.EnginePid -or $Snapshot.RunningState -ne 4 -or $Snapshot.ProcessExited) { return $false }
    if ($Snapshot.CreationTime -ne $Accepted.PidBoundCreationTime) { return $false }
    if ($Snapshot.Sid -ne $Accepted.PidBoundSid -or $Snapshot.Elevated -ne $Accepted.PidBoundElevated) { return $false }
    if ($Snapshot.ElevationType -ne $Accepted.PidBoundElevationType -or $Snapshot.IntegritySid -ne $Accepted.PidBoundIntegritySid) { return $false }
    return $true
}

function Invoke-RunAttemptObservation {
    param(
        [Parameter(Mandatory)] $Observation,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Instances,
        [Parameter(Mandatory)] [string] $ExpectedPath,
        [Parameter(Mandatory)] [string] $ExpectedActionId,
        [string] $ExpectedGuid = '',
        [int] $ExpectedPid = 0,
        $Accepted = $null,
        $Snapshot = $null,
        [switch] $CompleteAcceptance,
        [switch] $Final
    )
    $null = Update-RunAttemptObservation -Observation $Observation -Instances $Instances -ExpectedPath $ExpectedPath -ExpectedActionId $ExpectedActionId -ExpectedGuid $ExpectedGuid -ExpectedPid $ExpectedPid
    if ($Final) {
        if ($Instances.Count -ne 1) { $Observation.Rejections.Add('final observation was not a singleton') }
        elseif ($null -eq $Accepted -or $null -eq $Snapshot) { $Observation.Rejections.Add('final observation omitted the complete accepted process vector') }
        elseif ($CompleteAcceptance -and -not (Test-CompleteAgreement -Evidence $Accepted)) {
            $Observation.Rejections.Add('final observation started from incomplete accepted evidence')
        }
        elseif (-not (Test-StabilizedRunSnapshot -Accepted $Accepted -Snapshot $Snapshot)) {
            $Observation.Rejections.Add('final observation contradicted the complete accepted process vector')
        }
    }
    Assert-RunObservationAccepted -Observation $Observation -Attempt $(if ($Final) { 'final run observation' } else { 'run observation' })
    return $Observation
}

function Invoke-StopSettlement {
    param(
        [Parameter(Mandatory)] [scriptblock] $StopEffect,
        [Parameter(Mandatory)] [scriptblock] $ProcessesSettled,
        [Parameter(Mandatory)] [scriptblock] $InstancesSettled
    )
    $failures = [System.Collections.Generic.List[string]]::new()
    $stopOutcome = 'success'
    $stopDiagnostic = $null
    try { $null = & $StopEffect }
    catch {
        $stopOutcome = Get-CallAcknowledgement -Exception $_.Exception
        $stopDiagnostic = $_.Exception.Message
    }
    foreach ($probe in @(
        [pscustomobject]@{ Name = 'retained task processes'; Check = $ProcessesSettled },
        [pscustomobject]@{ Name = 'exact owned task instances'; Check = $InstancesSettled }
    )) {
        try {
            if (-not (& $($probe.Check))) { throw "$($probe.Name) did not settle" }
        }
        catch {
            $detail = if ($null -ne $stopDiagnostic) { "; stop call also failed: $stopDiagnostic" } else { '' }
            $failures.Add("$($_.Exception.Message)$detail")
        }
    }
    return [pscustomobject]@{
        StopCalled = $true
        StopOutcome = $stopOutcome
        StopDiagnostic = $stopDiagnostic
        Settled = $failures.Count -eq 0
        Failures = @($failures)
    }
}

function Invoke-DeleteSettlement {
    param(
        [Parameter(Mandatory)] [string] $Resource,
        [Parameter(Mandatory)] [scriptblock] $DeleteEffect,
        [Parameter(Mandatory)] [scriptblock] $ObservePresent
    )
    $deleteOutcome = 'success'
    $deleteDiagnostic = $null
    try { $null = & $DeleteEffect }
    catch {
        $deleteOutcome = Get-CallAcknowledgement -Exception $_.Exception
        $deleteDiagnostic = $_.Exception.Message
    }
    $observation = 'unobservable'
    $observationDiagnostic = $null
    try {
        if (& $ObservePresent) {
            $observation = 'present'
        }
        else {
            $observation = 'absent'
        }
    }
    catch { $observationDiagnostic = $_.Exception.Message }
    $failure = $null
    if ($observation -eq 'present') {
        $detail = if ($null -ne $deleteDiagnostic) { "; delete $deleteOutcome`: $deleteDiagnostic" } else { '' }
        $failure = "$Resource remained after deletion$detail"
    }
    elseif ($observation -eq 'unobservable') {
        $deleteDetail = if ($null -ne $deleteDiagnostic) { "; delete $deleteOutcome`: $deleteDiagnostic" } else { '' }
        $failure = "$Resource absence could not be observed: $observationDiagnostic$deleteDetail"
    }
    return [pscustomobject]@{
        DeleteOutcome = $deleteOutcome
        DeleteDiagnostic = $deleteDiagnostic
        Observation = $observation
        ObservationDiagnostic = $observationDiagnostic
        Settled = $observation -eq 'absent'
        Failure = $failure
    }
}

function Test-CreateEffectSettled {
    param($Acknowledgement, $Outcome)
    return $null -eq $Acknowledgement -or ($null -ne $Outcome -and $Outcome.Settled)
}

function Invoke-DependencyOrderedCleanup {
    param(
        [bool] $RunEffectsSettled,
        [bool] $TaskSettled,
        [bool] $TaskCleanupOwned,
        [Parameter(Mandatory)] [scriptblock] $DeleteTask,
        [bool] $FolderSettled,
        [bool] $FolderCleanupOwned,
        [Parameter(Mandatory)] [scriptblock] $DeleteFolder,
        [bool] $RootCleanupOwned,
        [Parameter(Mandatory)] [scriptblock] $DeleteRoot
    )
    $preservations = [System.Collections.Generic.List[string]]::new()
    if ($TaskCleanupOwned) {
        if ($RunEffectsSettled) { $TaskSettled = [bool](& $DeleteTask) }
        else { $preservations.Add('exact task was preserved because its processes or instances did not settle') }
    }
    if ($FolderCleanupOwned) {
        if ($TaskSettled) { $FolderSettled = [bool](& $DeleteFolder) }
        else { $preservations.Add('scheduler folder was preserved because its exact task did not settle') }
    }
    $rootSettled = -not $RootCleanupOwned
    if ($RootCleanupOwned) {
        if ($FolderSettled) { $rootSettled = [bool](& $DeleteRoot) }
        else { $preservations.Add('run root was preserved because its scheduler folder did not settle') }
    }
    return [pscustomobject]@{
        TaskSettled = $TaskSettled
        FolderSettled = $FolderSettled
        RootSettled = $rootSettled
        Preservations = @($preservations)
    }
}

function Format-ProofFailures {
    param($PrimaryFailure, [string[]] $CleanupFailures)
    $parts = [System.Collections.Generic.List[string]]::new()
    if ($null -ne $PrimaryFailure) { $parts.Add("primary failure: $($PrimaryFailure.Exception.Message)") }
    foreach ($failure in $CleanupFailures) { $parts.Add("cleanup failure: $failure") }
    return [string]::Join([Environment]::NewLine, $parts)
}

function Assert-LifecycleStateProbes {
    foreach ($observation in @('matching', 'contradictory', 'multiple', 'absent', 'unobservable')) {
        if (-not (Test-RunCleanupRequired -TaskOwned $true -RunAttempted $true -Observation $observation)) {
            throw "owned run cleanup was skipped for $observation"
        }
    }
    if (Test-RunCleanupRequired -TaskOwned $false -RunAttempted $true -Observation 'multiple') { throw 'foreign task gained stop authority' }
    $matchingInstance = [pscustomobject]@{ Path = '\planned\task'; CurrentAction = 'planned-action'; InstanceGuid = 'planned-guid'; EnginePID = 42 }
    foreach ($sequence in @(
        [pscustomobject]@{ Name = 'first run 2 to 1'; First = @($matchingInstance, $matchingInstance); Second = @($matchingInstance); Guid = ''; Pid = 0 },
        [pscustomobject]@{ Name = 'first run contradiction to match'; First = @([pscustomobject]@{ Path = '\wrong'; CurrentAction = 'planned-action'; InstanceGuid = 'planned-guid'; EnginePID = 42 }); Second = @($matchingInstance); Guid = ''; Pid = 0 },
        [pscustomobject]@{ Name = 'second run 2 to 1'; First = @($matchingInstance, $matchingInstance); Second = @($matchingInstance); Guid = 'planned-guid'; Pid = 42 },
        [pscustomobject]@{ Name = 'second run contradiction to match'; First = @([pscustomobject]@{ Path = '\planned\task'; CurrentAction = 'wrong-action'; InstanceGuid = 'planned-guid'; EnginePID = 42 }); Second = @($matchingInstance); Guid = 'planned-guid'; Pid = 42 }
    )) {
        $runObservation = New-RunAttemptObservation
        try { $null = Invoke-RunAttemptObservation -Observation $runObservation -Instances $sequence.First -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid $sequence.Guid -ExpectedPid $sequence.Pid }
        catch { }
        try { $null = Invoke-RunAttemptObservation -Observation $runObservation -Instances $sequence.Second -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid $sequence.Guid -ExpectedPid $sequence.Pid }
        catch { }
        if ($runObservation.Rejections.Count -eq 0) { throw "$($sequence.Name) forgot its earlier rejection" }
        $cleanupProbe = [pscustomobject]@{ StopCalls = 0 }
        $settlement = Invoke-StopSettlement -StopEffect { $cleanupProbe.StopCalls++ } -ProcessesSettled { $true } -InstancesSettled { $true }
        if ($cleanupProbe.StopCalls -ne 1 -or $settlement.Failures.Count -ne 0) { throw "$($sequence.Name) did not stop and settle its exact owned task" }
    }
    $finalObservation = New-RunAttemptObservation
    $null = Invoke-RunAttemptObservation -Observation $finalObservation -Instances @($matchingInstance) -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid 'planned-guid' -ExpectedPid 42
    $contradictoryFinal = [pscustomobject]@{ Path = '\planned\task'; CurrentAction = 'wrong-final-action'; InstanceGuid = 'planned-guid'; EnginePID = 42 }
    try { $null = Invoke-RunAttemptObservation -Observation $finalObservation -Instances @($contradictoryFinal) -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid 'planned-guid' -ExpectedPid 42 -Final }
    catch { }
    if ($finalObservation.Rejections.Count -eq 0) { throw 'matching poll followed by a contradictory final snapshot was accepted' }
    $omittedVectorObservation = New-RunAttemptObservation
    try { $null = Invoke-RunAttemptObservation -Observation $omittedVectorObservation -Instances @($matchingInstance) -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid 'planned-guid' -ExpectedPid 42 -Final }
    catch { }
    if ($omittedVectorObservation.Rejections -notcontains 'final observation omitted the complete accepted process vector') {
        throw 'matching final observation omitted its complete process vector without rejection'
    }
    $finalCleanupProbe = [pscustomobject]@{ StopCalls = 0 }
    $finalSettlement = Invoke-StopSettlement -StopEffect { $finalCleanupProbe.StopCalls++ } -ProcessesSettled { $true } -InstancesSettled { $true }
    if ($finalCleanupProbe.StopCalls -ne 1 -or $finalSettlement.Failures.Count -ne 0) { throw 'contradictory final snapshot did not stop and settle its exact owned task' }
    $acceptedSnapshot = [pscustomobject]@{
        CallerSid = 'S-1-5-21-1'; PrincipalSid = 'S-1-5-21-1'; TriggerSid = 'S-1-5-21-1'
        ActionSid = 'S-1-5-21-1'; PidBoundSid = 'S-1-5-21-1'
        CallerElevated = $false; ActionElevated = $false; PidBoundElevated = $false
        CallerElevationType = 2; ActionElevationType = 2; PidBoundElevationType = 2
        CallerIntegritySid = 'S-1-16-8192'; ActionIntegritySid = 'S-1-16-8192'; PidBoundIntegritySid = 'S-1-16-8192'
        ActionCreationTime = 100; PidBoundCreationTime = 100; ActionPid = 42; EnginePid = 42
        PlannedNonce = 'planned-nonce'; ActionNonce = 'planned-nonce'
        RunningTaskPath = '\planned\task'; CurrentAction = 'planned-action'
        PlannedTaskPath = '\planned\task'; PlannedActionId = 'planned-action'; InstanceGuid = 'planned-guid'
        InstanceCount = 1; RunningState = 4; EngineProcessExited = $false
        DefinitionMatches = $true; DescriptorMatches = $true; DaclProtected = $true
        Owner = 'S-1-5-21-1'; Group = 'S-1-5-21-1'; AceSids = 'S-1-5-18|S-1-5-21-1'
        AccessMasks = "$($script:FileAllAccess)|$($script:FileAllAccess)"; AceTypes = 'AccessAllowed|AccessAllowed'
        AceFlags = 'None|None'; ObjectAceCount = 0; AceCount = 2
    }
    $matchingSnapshot = [pscustomobject]@{
        InstanceCount = 1; Path = '\planned\task'; CurrentAction = 'planned-action'; InstanceGuid = 'planned-guid'
        EnginePid = 42; RunningState = 4; ProcessExited = $false; CreationTime = 100; Sid = 'S-1-5-21-1'
        Elevated = $false; ElevationType = 2; IntegritySid = 'S-1-16-8192'
    }
    $positiveObservation = New-RunAttemptObservation
    $null = Invoke-RunAttemptObservation -Observation $positiveObservation -Instances @($matchingInstance) -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid 'planned-guid' -ExpectedPid 42 -Accepted $acceptedSnapshot -Snapshot $matchingSnapshot -CompleteAcceptance -Final
    foreach ($mutation in @(
        [pscustomobject]@{ Field = 'Path'; Value = '\wrong-final-path' },
        [pscustomobject]@{ Field = 'CurrentAction'; Value = 'wrong-final-action' },
        [pscustomobject]@{ Field = 'ProcessExited'; Value = $true },
        [pscustomobject]@{ Field = 'CreationTime'; Value = 101 }
    )) {
        $copy = [ordered]@{}
        foreach ($property in $matchingSnapshot.PSObject.Properties) { $copy[$property.Name] = $property.Value }
        $copy[$mutation.Field] = $mutation.Value
        $negativeObservation = New-RunAttemptObservation
        try { $null = Invoke-RunAttemptObservation -Observation $negativeObservation -Instances @($matchingInstance) -ExpectedPath '\planned\task' -ExpectedActionId 'planned-action' -ExpectedGuid 'planned-guid' -ExpectedPid 42 -Accepted $acceptedSnapshot -Snapshot ([pscustomobject]$copy) -CompleteAcceptance -Final }
        catch { }
        if ($negativeObservation.Rejections.Count -eq 0) { throw "stabilized snapshot accepted mutation of $($mutation.Field)" }
    }
    $lostStop = Invoke-StopSettlement -StopEffect { throw [System.Management.Automation.MethodInvocationException]::new('lost stop reply', [System.Runtime.InteropServices.COMException]::new('lost stop reply', -2147023170)) } -ProcessesSettled { $true } -InstancesSettled { $true }
    if (-not $lostStop.StopCalled -or $lostStop.StopOutcome -ne 'lost' -or -not $lostStop.Settled -or $lostStop.Failures.Count -ne 0) { throw 'lost stop reply did not settle through fresh observations' }
    $failedStop = Invoke-StopSettlement -StopEffect { throw 'stop rejected' } -ProcessesSettled { $false } -InstancesSettled { $false }
    if ($failedStop.Settled -or $failedStop.Failures.Count -ne 2 -or $failedStop.Failures[0] -notmatch 'stop rejected') { throw 'stop failure and lingering effects were not retained' }
    foreach ($scenario in @(
        [pscustomobject]@{ Name = 'unsettled run'; Run = $false; TaskSettled = $false; TaskOwned = $true; TaskDelete = $true; FolderSettled = $false; FolderOwned = $true; FolderDelete = $true; RootOwned = $true; Expected = '0|0|0'; Preservations = 3 },
        [pscustomobject]@{ Name = 'preserved task create effect'; Run = $true; TaskSettled = $false; TaskOwned = $false; TaskDelete = $true; FolderSettled = $false; FolderOwned = $true; FolderDelete = $true; RootOwned = $true; Expected = '0|0|0'; Preservations = 2 },
        [pscustomobject]@{ Name = 'uncertain folder create effect'; Run = $true; TaskSettled = $true; TaskOwned = $false; TaskDelete = $true; FolderSettled = $false; FolderOwned = $false; FolderDelete = $true; RootOwned = $true; Expected = '0|0|0'; Preservations = 1 },
        [pscustomobject]@{ Name = 'task deletion did not settle'; Run = $true; TaskSettled = $false; TaskOwned = $true; TaskDelete = $false; FolderSettled = $false; FolderOwned = $true; FolderDelete = $true; RootOwned = $true; Expected = '1|0|0'; Preservations = 2 },
        [pscustomobject]@{ Name = 'folder deletion did not settle'; Run = $true; TaskSettled = $true; TaskOwned = $false; TaskDelete = $true; FolderSettled = $false; FolderOwned = $true; FolderDelete = $false; RootOwned = $true; Expected = '0|1|0'; Preservations = 1 },
        [pscustomobject]@{ Name = 'lost stop reply with confirmed settlement'; Run = $lostStop.Settled; TaskSettled = $false; TaskOwned = $true; TaskDelete = $true; FolderSettled = $false; FolderOwned = $true; FolderDelete = $true; RootOwned = $true; Expected = '1|1|1'; Preservations = 0 }
    )) {
        $calls = [pscustomobject]@{ Task = 0; Folder = 0; Root = 0 }
        $ordered = Invoke-DependencyOrderedCleanup -RunEffectsSettled $scenario.Run -TaskSettled $scenario.TaskSettled -TaskCleanupOwned $scenario.TaskOwned -DeleteTask {
            $calls.Task++
            return $scenario.TaskDelete
        } -FolderSettled $scenario.FolderSettled -FolderCleanupOwned $scenario.FolderOwned -DeleteFolder {
            $calls.Folder++
            return $scenario.FolderDelete
        } -RootCleanupOwned $scenario.RootOwned -DeleteRoot {
            $calls.Root++
            return $true
        }
        $actualCalls = "$($calls.Task)|$($calls.Folder)|$($calls.Root)"
        if ($actualCalls -ne $scenario.Expected) { throw "$($scenario.Name) cleanup order was $actualCalls, expected $($scenario.Expected)" }
        if ($ordered.Preservations.Count -ne $scenario.Preservations) { throw "$($scenario.Name) retained $($ordered.Preservations.Count) preservation facts, expected $($scenario.Preservations)" }
        if ($scenario.Expected -eq '1|1|1' -and (-not $ordered.TaskSettled -or -not $ordered.FolderSettled -or -not $ordered.RootSettled)) {
            throw 'confirmed settled lost stop reply did not permit ordered cleanup'
        }
    }
    $deleteEffects = @(
        [pscustomobject]@{ Name = 'success'; Effect = { } },
        [pscustomobject]@{ Name = 'lost'; Effect = { throw [System.Management.Automation.MethodInvocationException]::new('lost delete reply', [System.Runtime.InteropServices.COMException]::new('lost delete reply', -2147023170)) } },
        [pscustomobject]@{ Name = 'rejected'; Effect = { throw [System.Management.Automation.MethodInvocationException]::new('delete rejected', [System.Runtime.InteropServices.COMException]::new('delete rejected', -2147024891)) } }
    )
    $deleteObservations = @(
        [pscustomobject]@{ Name = 'absent'; Observe = { $false } },
        [pscustomobject]@{ Name = 'present'; Observe = { $true } },
        [pscustomobject]@{ Name = 'unobservable'; Observe = { throw 'observation failed' } }
    )
    foreach ($effect in $deleteEffects) {
        foreach ($fresh in $deleteObservations) {
            $settlement = Invoke-DeleteSettlement -Resource 'task' -DeleteEffect $effect.Effect -ObservePresent $fresh.Observe
            if ($settlement.DeleteOutcome -ne $effect.Name -or $settlement.Observation -ne $fresh.Name) {
                throw "delete matrix $($effect.Name)/$($fresh.Name) lost a typed fact"
            }
            if ($fresh.Name -eq 'absent') {
                if (-not $settlement.Settled -or $null -ne $settlement.Failure) { throw "delete matrix $($effect.Name)/absent did not settle" }
            }
            elseif ($null -eq $settlement.Failure) {
                throw "delete matrix $($effect.Name)/$($fresh.Name) lost its cleanup failure"
            }
            if ($fresh.Name -eq 'unobservable' -and $effect.Name -ne 'success') {
                $expectedDeleteDiagnostic = if ($effect.Name -eq 'lost') { 'lost delete reply' } else { 'delete rejected' }
                if ($settlement.Failure -notmatch $expectedDeleteDiagnostic) {
                    throw "delete matrix $($effect.Name)/unobservable lost the delete diagnostic"
                }
            }
        }
    }
    try { throw 'primary' }
    catch { $primary = $_ }
    $combined = Format-ProofFailures -PrimaryFailure $primary -CleanupFailures @('task remained', 'folder unobservable')
    if ($combined -notmatch 'primary failure: primary[\s\S]*cleanup failure: task remained[\s\S]*cleanup failure: folder unobservable') {
        throw 'primary and ordered cleanup failures were not retained together'
    }
}

function ConvertTo-ExactDescriptor {
    param([Parameter(Mandatory)] [string] $Sddl)
    $descriptor = [System.Security.AccessControl.RawSecurityDescriptor]::new($Sddl)
    $aces = @()
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        $aces += [pscustomobject]@{
            Type = [string]$ace.AceType
            Flags = [string]$ace.AceFlags
            Mask = $ace.AccessMask
            Sid = $ace.SecurityIdentifier.Value
            IsObject = $ace -is [System.Security.AccessControl.ObjectAce]
        }
    }
    return [pscustomobject]@{
        Owner = $descriptor.Owner.Value
        Group = $descriptor.Group.Value
        Protected = ($descriptor.ControlFlags -band [System.Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -ne 0
        Aces = $aces
    }
}

function Test-ExactDescriptor {
    param(
        [Parameter(Mandatory)] [string] $Sddl,
        [Parameter(Mandatory)] [string] $CallerSid
    )
    $facts = ConvertTo-ExactDescriptor -Sddl $Sddl
    if ($facts.Owner -ne $CallerSid -or $facts.Group -ne $CallerSid -or -not $facts.Protected) { return $false }
    if ($facts.Aces.Count -ne 2) { return $false }
    $expectedSids = @($CallerSid, $script:LocalSystemSid) | Sort-Object
    $actualSids = @($facts.Aces | ForEach-Object { $_.Sid }) | Sort-Object
    if ([string]::Join('|', $expectedSids) -ne [string]::Join('|', $actualSids)) { return $false }
    foreach ($ace in $facts.Aces) {
        if ($ace.Type -ne 'AccessAllowed' -or $ace.Flags -ne 'None' -or $ace.Mask -ne $script:FileAllAccess -or $ace.IsObject) {
            return $false
        }
    }
    return $true
}

function Get-FileDescriptor {
    param([Parameter(Mandatory)] [string] $Path)
    $security = [System.IO.Directory]::GetAccessControl(
        $Path,
        [System.Security.AccessControl.AccessControlSections]::Owner -bor
            [System.Security.AccessControl.AccessControlSections]::Group -bor
            [System.Security.AccessControl.AccessControlSections]::Access
    )
    return $security.GetSecurityDescriptorSddlForm([System.Security.AccessControl.AccessControlSections]::All)
}

function Get-ExactFolder {
    param($RootFolder, [string] $Path)
    try { return $RootFolder.GetFolder($Path) }
    catch {
        if (Test-NotFound -Exception $_.Exception) { return $null }
        throw "folder observation failed at $Path with $(Get-HResultHex -Exception $_.Exception): $($_.Exception.Message)"
    }
}

function Get-ExactTask {
    param($Folder, [string] $Name)
    try { return $Folder.GetTask($Name) }
    catch {
        if (Test-NotFound -Exception $_.Exception) { return $null }
        throw "task observation failed at $Name with $(Get-HResultHex -Exception $_.Exception): $($_.Exception.Message)"
    }
}

function Resolve-AccountSid {
    param([string] $Account)
    if ([string]::IsNullOrWhiteSpace($Account)) { return $Account }
    if ($Account -match '^S-1-\d+(-\d+)+$') { return $Account }
    try {
        return ([System.Security.Principal.NTAccount]::new($Account)).Translate([System.Security.Principal.SecurityIdentifier]).Value
    }
    catch {
        return $Account
    }
}

# Each disagreement is named with both values, so a native contradiction says
# which fact Task Scheduler read back differently rather than only that one did.
function Get-TaskDefinitionMismatches {
    param(
        $Task,
        [string] $CallerSid,
        [string] $ActionId,
        [string] $Executable,
        [string] $Arguments,
        [string] $WorkingDirectory,
        [string] $RegistrationSource
    )
    $mismatches = [System.Collections.Generic.List[string]]::new()
    function Compare-Fact([string] $Name, $Observed, $Planned) {
        if ($Observed -ne $Planned) { $mismatches.Add("$Name observed '$Observed' planned '$Planned'") }
    }
    $definition = $Task.Definition
    # Registered with the caller's SID, Task Scheduler reads UserId back as an
    # account name: bare for the principal, machine-qualified for the trigger
    # (run 36226784863). Identity is the SID that name resolves to, never the
    # name's spelling; an unresolvable name is kept as observed and disagrees.
    Compare-Fact 'Principal.UserId' (Resolve-AccountSid $definition.Principal.UserId) $CallerSid
    Compare-Fact 'Principal.LogonType' $definition.Principal.LogonType 3
    Compare-Fact 'Principal.RunLevel' $definition.Principal.RunLevel 0
    Compare-Fact 'Triggers.Count' $definition.Triggers.Count 1
    if ($definition.Triggers.Count -eq 1) {
        $trigger = $definition.Triggers.Item(1)
        Compare-Fact 'Trigger.Type' $trigger.Type 9
        Compare-Fact 'Trigger.Enabled' $trigger.Enabled $true
        Compare-Fact 'Trigger.UserId' (Resolve-AccountSid $trigger.UserId) $CallerSid
    }
    Compare-Fact 'Actions.Count' $definition.Actions.Count 1
    if ($definition.Actions.Count -eq 1) {
        $action = $definition.Actions.Item(1)
        Compare-Fact 'Action.Type' $action.Type 0
        Compare-Fact 'Action.Id' $action.Id $ActionId
        Compare-Fact 'Action.Path' $action.Path $Executable
        Compare-Fact 'Action.Arguments' $action.Arguments $Arguments
        Compare-Fact 'Action.WorkingDirectory' $action.WorkingDirectory $WorkingDirectory
    }
    $settings = $definition.Settings
    Compare-Fact 'Settings.MultipleInstances' $settings.MultipleInstances 2
    Compare-Fact 'Settings.AllowDemandStart' $settings.AllowDemandStart $true
    Compare-Fact 'Settings.ExecutionTimeLimit' $settings.ExecutionTimeLimit 'PT0S'
    Compare-Fact 'Settings.DisallowStartIfOnBatteries' $settings.DisallowStartIfOnBatteries $false
    Compare-Fact 'Settings.StopIfGoingOnBatteries' $settings.StopIfGoingOnBatteries $false
    Compare-Fact 'Settings.RunOnlyIfIdle' $settings.RunOnlyIfIdle $false
    Compare-Fact 'Settings.RunOnlyIfNetworkAvailable' $settings.RunOnlyIfNetworkAvailable $false
    Compare-Fact 'Settings.RestartCount' $settings.RestartCount 3
    Compare-Fact 'Settings.RestartInterval' $settings.RestartInterval 'PT1M'
    Compare-Fact 'RegistrationInfo.Source' $definition.RegistrationInfo.Source $RegistrationSource
    return , $mismatches.ToArray()
}

function Test-TaskDefinition {
    param(
        $Task,
        [string] $CallerSid,
        [string] $ActionId,
        [string] $Executable,
        [string] $Arguments,
        [string] $WorkingDirectory,
        [string] $RegistrationSource
    )
    return (Get-TaskDefinitionMismatches @PSBoundParameters).Count -eq 0
}

function Wait-Until {
    param(
        [Parameter(Mandatory)] [scriptblock] $Condition,
        [Parameter(Mandatory)] [string] $Description,
        [int] $TimeoutMilliseconds = 20000
    )
    $deadline = [System.Diagnostics.Stopwatch]::StartNew()
    while ($deadline.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "timed out waiting for $Description"
}

function Get-TaskInstances {
    param($Task)
    $instances = @()
    $collection = $Task.GetInstances(0)
    for ($index = 1; $index -le $collection.Count; $index++) {
        $instance = $collection.Item($index)
        $instances += $instance
        if ($instance.EnginePID -ne 0 -and -not $script:ObservedProcesses.ContainsKey([int]$instance.EnginePID)) {
            try {
                $process = [NessaWindowsProofNative]::OpenProcessForObservation([uint32]$instance.EnginePID)
                $script:ObservedProcesses.Add([int]$instance.EnginePID, $process)
            }
            catch {
                throw "could not retain EnginePID $($instance.EnginePID) from exact owned task: $($_.Exception.Message)"
            }
        }
    }
    return $instances
}

# Field names, not text: a benign temp path containing "secret" is an action
# argument, and a text match can never show that a value is not a credential.
# The principal of an interactive-token task names a user, its logon type, and
# at most a run level; the typed definition check owns their values.
function Test-TaskXmlCarriesNoCredential {
    param([Parameter(Mandatory)] [string] $Xml)
    $document = [xml]$Xml
    foreach ($node in $document.SelectNodes('//* | //@*')) {
        if ($node.LocalName -match '(?i)password|credential|secret') { return $false }
    }
    $principals = @($document.SelectNodes("//*[local-name()='Principals']/*[local-name()='Principal']"))
    if ($principals.Count -ne 1) { return $false }
    $children = @($principals[0].ChildNodes | Where-Object { $_ -is [System.Xml.XmlElement] } | ForEach-Object { $_.LocalName })
    if ($children -notcontains 'UserId' -or $children -notcontains 'LogonType') { return $false }
    foreach ($child in $children) {
        if ($child -notin @('UserId', 'LogonType', 'RunLevel')) { return $false }
    }
    return $children.Count -eq @($children | Sort-Object -Unique).Count
}

function Assert-CredentialFieldProbes {
    $principal = '<Principals><Principal id="Author"><UserId>S-1-5-21-1</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>'
    $benign = "<Task xmlns='http://schemas.microsoft.com/windows/2004/02/mit/task'>$principal<Actions><Exec><Arguments>-File C:\secret-credential-password\a.ps1</Arguments></Exec></Actions></Task>"
    if (-not (Test-TaskXmlCarriesNoCredential -Xml $benign)) { throw 'credential probe rejected a benign path that only mentions a credential word' }
    $withPassword = $benign.Replace('<RunLevel>', '<Password>x</Password><RunLevel>')
    if (Test-TaskXmlCarriesNoCredential -Xml $withPassword) { throw 'credential probe accepted a Password element' }
    $withAttribute = $benign.Replace('<Exec>', '<Exec credential="x">')
    if (Test-TaskXmlCarriesNoCredential -Xml $withAttribute) { throw 'credential probe accepted a credential attribute' }
    $withExtraChild = $benign.Replace('<RunLevel>', '<GroupId>S-1-5-32-545</GroupId><RunLevel>')
    if (Test-TaskXmlCarriesNoCredential -Xml $withExtraChild) { throw 'credential probe accepted an unexpected principal element' }
}

function Test-CompleteAgreement {
    param([Parameter(Mandatory)] $Evidence)
    $equalFields = @(
        @('CallerSid', 'PrincipalSid'), @('CallerSid', 'TriggerSid'),
        @('CallerSid', 'ActionSid'), @('CallerSid', 'PidBoundSid'),
        @('CallerElevated', 'ActionElevated'), @('CallerElevated', 'PidBoundElevated'),
        @('CallerElevationType', 'ActionElevationType'), @('CallerElevationType', 'PidBoundElevationType'),
        @('CallerIntegritySid', 'ActionIntegritySid'), @('CallerIntegritySid', 'PidBoundIntegritySid'),
        @('ActionCreationTime', 'PidBoundCreationTime'),
        @('ActionPid', 'EnginePid'), @('PlannedNonce', 'ActionNonce'),
        @('PlannedTaskPath', 'RunningTaskPath'), @('PlannedActionId', 'CurrentAction')
    )
    foreach ($pair in $equalFields) {
        if ($Evidence.($pair[0]) -ne $Evidence.($pair[1])) { return $false }
    }
    if ([string]::IsNullOrWhiteSpace([string]$Evidence.InstanceGuid)) { return $false }
    if ($Evidence.InstanceCount -ne 1 -or $Evidence.RunningState -ne 4 -or $Evidence.EnginePid -eq 0 -or $Evidence.EngineProcessExited) { return $false }
    if (-not $Evidence.DefinitionMatches -or -not $Evidence.DescriptorMatches -or -not $Evidence.DaclProtected) { return $false }
    if ($Evidence.Owner -ne $Evidence.CallerSid -or $Evidence.Group -ne $Evidence.CallerSid -or $Evidence.AceCount -ne 2) { return $false }
    $expectedAceSids = @($Evidence.CallerSid, $script:LocalSystemSid) | Sort-Object
    if ($Evidence.AceSids -ne [string]::Join('|', $expectedAceSids)) { return $false }
    if ($Evidence.AccessMasks -ne "$($script:FileAllAccess)|$($script:FileAllAccess)" -or $Evidence.AceTypes -ne 'AccessAllowed|AccessAllowed') { return $false }
    if ($Evidence.AceFlags -ne 'None|None' -or $Evidence.ObjectAceCount -ne 0) { return $false }
    return $true
}

function Assert-NegativeEvidenceProbes {
    param([Parameter(Mandatory)] $Accepted)
    $matchingInstance = [pscustomobject]@{
        Path = $Accepted.PlannedTaskPath; CurrentAction = $Accepted.PlannedActionId
        InstanceGuid = $Accepted.InstanceGuid; EnginePID = $Accepted.EnginePid
    }
    $matchingSnapshot = [pscustomobject]@{
        InstanceCount = 1; Path = $Accepted.PlannedTaskPath; CurrentAction = $Accepted.PlannedActionId
        InstanceGuid = $Accepted.InstanceGuid; EnginePid = $Accepted.EnginePid; RunningState = 4; ProcessExited = $false
        CreationTime = $Accepted.PidBoundCreationTime; Sid = $Accepted.PidBoundSid; Elevated = $Accepted.PidBoundElevated
        ElevationType = $Accepted.PidBoundElevationType; IntegritySid = $Accepted.PidBoundIntegritySid
    }
    $positiveObservation = New-RunAttemptObservation
    $null = Invoke-RunAttemptObservation -Observation $positiveObservation -Instances @($matchingInstance) -ExpectedPath $Accepted.PlannedTaskPath -ExpectedActionId $Accepted.PlannedActionId -ExpectedGuid $Accepted.InstanceGuid -ExpectedPid $Accepted.EnginePid -Accepted $Accepted -Snapshot $matchingSnapshot -CompleteAcceptance -Final
    $mutations = @{
        ActionSid = 'S-1-5-18'; ActionElevated = -not $Accepted.ActionElevated; ActionElevationType = -1
        ActionIntegritySid = 'S-1-16-0'; PidBoundSid = 'S-1-5-18'; PidBoundElevated = -not $Accepted.PidBoundElevated
        PidBoundElevationType = -1; PidBoundIntegritySid = 'S-1-16-0'; ActionPid = $Accepted.ActionPid + 1
        ActionCreationTime = $Accepted.ActionCreationTime + 1; PidBoundCreationTime = $Accepted.PidBoundCreationTime + 1
        ActionNonce = "$($Accepted.ActionNonce)-wrong"; RunningTaskPath = "$($Accepted.RunningTaskPath)-wrong"
        InstanceGuid = ''; InstanceCount = 2; RunningState = 3; EnginePid = 0; EngineProcessExited = $true
        CurrentAction = "$($Accepted.CurrentAction)-wrong"; PrincipalSid = 'S-1-5-18'
        TriggerSid = 'S-1-5-18'; DefinitionMatches = $false; DescriptorMatches = $false; DaclProtected = $false
        Owner = 'S-1-5-18'; Group = 'S-1-5-18'; AceSids = 'S-1-5-18|S-1-5-18'
        AccessMasks = '1|1'; AceTypes = 'AccessAllowed|AccessDenied'; AceFlags = 'Inherited|None'
        ObjectAceCount = 1; AceCount = 3
    }
    foreach ($entry in $mutations.GetEnumerator()) {
        $copy = [ordered]@{}
        foreach ($property in $Accepted.PSObject.Properties) { $copy[$property.Name] = $property.Value }
        $copy[$entry.Key] = $entry.Value
        $negativeObservation = New-RunAttemptObservation
        try { $null = Invoke-RunAttemptObservation -Observation $negativeObservation -Instances @($matchingInstance) -ExpectedPath $Accepted.PlannedTaskPath -ExpectedActionId $Accepted.PlannedActionId -ExpectedGuid $Accepted.InstanceGuid -ExpectedPid $Accepted.EnginePid -Accepted ([pscustomobject]$copy) -Snapshot $matchingSnapshot -CompleteAcceptance -Final }
        catch { }
        if ($negativeObservation.Rejections -notcontains 'final observation started from incomplete accepted evidence') {
            throw "complete observation owner accepted mutation of $($entry.Key)"
        }
    }
}

function Add-CleanupFailure {
    param([Parameter(Mandatory)] [string] $Message)
    $script:CleanupFailures.Add($Message)
}

if ($env:OS -ne 'Windows_NT') {
    throw 'the Task Scheduler native model proof requires Windows'
}

Assert-LedgerMatrix
Assert-HResultClassification
Assert-LifecycleStateProbes
Assert-CredentialFieldProbes
$caller = [NessaWindowsProofNative]::ReadCurrentProcessTokenFacts()
if ($CallerContext -eq 'StandardUser' -and $caller.Elevated) { throw 'the Windows runner process is elevated; the least-privilege current-user model is not proved' }
if ($CallerContext -eq 'Administrator' -and -not $caller.Elevated) { throw 'the declared Administrator caller context is not elevated; this run is not in the environment it declares' }
Write-Host "Caller context ${CallerContext}: SID $($caller.Sid), elevated $($caller.Elevated), elevation type $($caller.ElevationType), integrity $($caller.IntegritySid)"

$runId = if ($env:GITHUB_RUN_ID) { $env:GITHUB_RUN_ID } else { 'local' }
$attempt = if ($env:GITHUB_RUN_ATTEMPT) { $env:GITHUB_RUN_ATTEMPT } else { '0' }
$nonce = [Guid]::NewGuid().ToString('N')
$scope = "nessa-task-proof-$runId-$attempt-$PID-$nonce"
$parent = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$parent = [System.IO.Path]::GetFullPath($parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
$parentInfo = [System.IO.DirectoryInfo]::new($parent)
if (-not $parentInfo.Exists -or ($parentInfo.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
    throw "proof parent is absent or a reparse point: $parent"
}
$runRoot = [System.IO.Path]::Combine($parent, $scope)
$folderName = $scope
$folderPath = "\$folderName"
$taskName = 'gateway-model'
$taskPath = "$folderPath\$taskName"
$actionId = "action-$nonce"
$registrationSource = "Nessa WS8 #205 $nonce"
$fixturePath = Join-Path $runRoot 'task-action.ps1'
$evidencePath = Join-Path $runRoot 'action-evidence.json'
$callerSid = $caller.Sid
$sddl = "O:${callerSid}G:${callerSid}D:P(A;;FA;;;${callerSid})(A;;FA;;;SY)"
$securityInformation = 1 -bor 2 -bor 4

$rootOwned = $false
$folderOwned = $false
$taskOwned = $false
$runAttempted = $false
$service = $null
$schedulerRoot = $null
$ownedFolder = $null
$ownedTask = $null
$folderAcknowledgement = $null
$taskAcknowledgement = $null
$folderOutcome = $null
$taskOutcome = $null
$primaryFailure = $null
$runRootHandle = $null
$runRootIdentity = $null

try {
    if ([System.IO.Directory]::Exists($runRoot)) { throw "nonce run root already exists: $runRoot" }
    $rootCreateError = [NessaWindowsProofNative]::CreatePrivateDirectory($runRoot, $sddl)
    if ($rootCreateError -ne 0) {
        throw "CreateDirectoryW failed for $runRoot with Win32 error $rootCreateError"
    }
    $rootOwned = $true
    $runRootHandle = [NessaWindowsProofNative]::OpenDirectory($runRoot)
    if ([NessaWindowsProofNative]::DirectoryIsReparsePoint($runRootHandle)) { throw 'created run root handle identifies a reparse point' }
    $runRootIdentity = [NessaWindowsProofNative]::DirectoryIdentity($runRootHandle)
    $rootItem = Get-Item -LiteralPath $runRoot -Force
    if ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { throw 'created run root is a reparse point' }
    if ([System.IO.Path]::GetFullPath($rootItem.Parent.FullName).TrimEnd('\') -ne $parent.TrimEnd('\')) { throw 'created run root has the wrong canonical parent' }
    if (-not (Test-ExactDescriptor -Sddl (Get-FileDescriptor -Path $runRoot) -CallerSid $callerSid)) { throw 'created run root security descriptor contradicts the plan' }
    $fixture = @'
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Nonce, [Parameter(Mandatory)][string]$EvidencePath)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class NessaActionToken {
    const uint TOKEN_QUERY=8; const int TokenUser=1, TokenElevationType=18, TokenElevation=20, TokenIntegrityLevel=25;
    [StructLayout(LayoutKind.Sequential)] struct SA { public IntPtr Sid; public uint Attributes; }
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetProcessTimes(IntPtr p,out System.Runtime.InteropServices.ComTypes.FILETIME c,out System.Runtime.InteropServices.ComTypes.FILETIME e,out System.Runtime.InteropServices.ComTypes.FILETIME k,out System.Runtime.InteropServices.ComTypes.FILETIME u);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr h);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool OpenProcessToken(IntPtr p,uint a,out IntPtr t);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetTokenInformation(IntPtr t,int c,IntPtr b,int n,out int r);
    [DllImport("advapi32.dll",SetLastError=true)] static extern bool ConvertSidToStringSidW(IntPtr s,out IntPtr p);
    static byte[] Info(IntPtr t,int c){int n;GetTokenInformation(t,c,IntPtr.Zero,0,out n);var b=new byte[n];var g=GCHandle.Alloc(b,GCHandleType.Pinned);try{if(!GetTokenInformation(t,c,g.AddrOfPinnedObject(),n,out n))throw new Win32Exception(Marshal.GetLastWin32Error());return b;}finally{g.Free();}}
    static string TokenSid(IntPtr t,int c){var b=Info(t,c);var g=GCHandle.Alloc(b,GCHandleType.Pinned);try{var v=(SA)Marshal.PtrToStructure(g.AddrOfPinnedObject(),typeof(SA));return Sid(v.Sid);}finally{g.Free();}}
    static string Sid(IntPtr s){IntPtr p;if(!ConvertSidToStringSidW(s,out p))throw new Win32Exception(Marshal.GetLastWin32Error());try{return Marshal.PtrToStringUni(p);}finally{LocalFree(p);}}
    static long Creation(){System.Runtime.InteropServices.ComTypes.FILETIME c,e,k,u;if(!GetProcessTimes(GetCurrentProcess(),out c,out e,out k,out u))throw new Win32Exception(Marshal.GetLastWin32Error());return((long)(uint)c.dwHighDateTime<<32)|(uint)c.dwLowDateTime;}
    public static string Read(){IntPtr t;if(!OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,out t))throw new Win32Exception(Marshal.GetLastWin32Error());try{return String.Join("|",TokenSid(t,TokenUser),(BitConverter.ToInt32(Info(t,TokenElevation),0)!=0).ToString(),BitConverter.ToInt32(Info(t,TokenElevationType),0),TokenSid(t,TokenIntegrityLevel),Creation());}finally{CloseHandle(t);}}
}
"@
$parts = [NessaActionToken]::Read().Split('|')
$record = [ordered]@{ nonce=$Nonce; pid=$PID; sid=$parts[0]; elevated=[bool]::Parse($parts[1]); elevationType=[int]$parts[2]; integritySid=$parts[3]; creationTime=[long]$parts[4] }
$temporary = "$EvidencePath.$PID.tmp"
$bytes = [System.Text.Encoding]::UTF8.GetBytes(($record | ConvertTo-Json -Compress))
$stream = [System.IO.File]::Open($temporary,[System.IO.FileMode]::CreateNew,[System.IO.FileAccess]::Write,[System.IO.FileShare]::None)
try { $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
[System.IO.File]::Move($temporary,$EvidencePath)
while ($true) { Start-Sleep -Milliseconds 100 }
'@
    [System.IO.File]::WriteAllText($fixturePath, $fixture, [System.Text.UTF8Encoding]::new($false))

    $service = New-Object -ComObject 'Schedule.Service'
    $service.Connect()
    $schedulerRoot = $service.GetFolder('\')
    if ($null -ne (Get-ExactFolder -RootFolder $schedulerRoot -Path $folderPath)) { throw "scheduler folder already exists: $folderPath" }
    $folderAcknowledgement = 'success'
    try { $null = $schedulerRoot.CreateFolder($folderName, $sddl) }
    catch {
        $folderAcknowledgement = Get-CallAcknowledgement -Exception $_.Exception
        $folderFailure = $_
    }
    $observedFolder = Get-ExactFolder -RootFolder $schedulerRoot -Path $folderPath
    if ($null -eq $observedFolder) { throw "scheduler folder creation was $folderAcknowledgement and the folder is absent" }
    $folderMatches = Test-ExactDescriptor -Sddl $observedFolder.GetSecurityDescriptor($securityInformation) -CallerSid $callerSid
    $folderOutcome = Resolve-CreateOutcome -Acknowledgement $folderAcknowledgement -Observation $(if ($folderMatches) { 'matching' } else { 'contradictory' })
    if ($folderOutcome.CleanupOwned) {
        $folderOwned = $true
        $ownedFolder = $observedFolder
    }
    if ($folderAcknowledgement -ne 'success') { throw "scheduler folder creation was $folderAcknowledgement`: $folderFailure" }
    if (-not $folderMatches) { throw 'scheduler folder descriptor contradicts the plan' }

    if ($null -ne (Get-ExactTask -Folder $ownedFolder -Name $taskName)) { throw "scheduler task already exists: $taskPath" }
    $powershell = Join-Path $PSHOME 'powershell.exe'
    $arguments = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$fixturePath`" -Nonce `"$nonce`" -EvidencePath `"$evidencePath`""
    $definition = $service.NewTask(0)
    $definition.RegistrationInfo.Description = 'Nessa Windows Task Scheduler model proof'
    $definition.RegistrationInfo.Source = $registrationSource
    $definition.Principal.UserId = $callerSid
    $definition.Principal.LogonType = 3
    $definition.Principal.RunLevel = 0
    $trigger = $definition.Triggers.Create(9)
    $trigger.Id = "logon-$nonce"
    $trigger.UserId = $callerSid
    $trigger.Enabled = $true
    $action = $definition.Actions.Create(0)
    $action.Id = $actionId
    $action.Path = $powershell
    $action.Arguments = $arguments
    $action.WorkingDirectory = $runRoot
    $settings = $definition.Settings
    $settings.MultipleInstances = 2
    $settings.AllowDemandStart = $true
    $settings.ExecutionTimeLimit = 'PT0S'
    $settings.DisallowStartIfOnBatteries = $false
    $settings.StopIfGoingOnBatteries = $false
    $settings.RunOnlyIfIdle = $false
    $settings.RunOnlyIfNetworkAvailable = $false
    $settings.RestartCount = 3
    $settings.RestartInterval = 'PT1M'

    $taskAcknowledgement = 'success'
    try { $null = $ownedFolder.RegisterTaskDefinition($taskName, $definition, 2 -bor 16, $callerSid, $null, 3, $sddl) }
    catch {
        $taskAcknowledgement = Get-CallAcknowledgement -Exception $_.Exception
        $taskFailure = $_
    }
    $observedTask = Get-ExactTask -Folder $ownedFolder -Name $taskName
    if ($null -eq $observedTask) { throw "task registration was $taskAcknowledgement and the task is absent" }
    $taskDefinitionMatches = Test-TaskDefinition -Task $observedTask -CallerSid $callerSid -ActionId $actionId -Executable $powershell -Arguments $arguments -WorkingDirectory $runRoot -RegistrationSource $registrationSource
    $taskDescriptorMatches = Test-ExactDescriptor -Sddl $observedTask.GetSecurityDescriptor($securityInformation) -CallerSid $callerSid
    $taskOutcome = Resolve-CreateOutcome -Acknowledgement $taskAcknowledgement -Observation $(if ($taskDefinitionMatches -and $taskDescriptorMatches) { 'matching' } else { 'contradictory' })
    if ($taskOutcome.CleanupOwned) {
        $taskOwned = $true
        $ownedTask = $observedTask
    }
    if ($taskAcknowledgement -ne 'success') { throw "task registration was $taskAcknowledgement`: $taskFailure" }
    if (-not $taskDefinitionMatches) {
        $mismatches = Get-TaskDefinitionMismatches -Task $observedTask -CallerSid $callerSid -ActionId $actionId -Executable $powershell -Arguments $arguments -WorkingDirectory $runRoot -RegistrationSource $registrationSource
        throw "registered task definition contradicts the plan: $([string]::Join('; ', $mismatches))"
    }
    if (-not $taskDescriptorMatches) { throw 'registered task descriptor contradicts the plan' }
    if (-not (Test-TaskXmlCarriesNoCredential -Xml $observedTask.Xml)) { throw 'registered task XML carries a credential field or an unexpected principal element' }

    $runAttempted = $true
    $firstReturned = $ownedTask.Run($null)
    $firstRunObservation = New-RunAttemptObservation
    Wait-Until -Description 'one running task instance and action evidence' -Condition {
        $instances = @(Get-TaskInstances -Task $ownedTask)
        $null = Invoke-RunAttemptObservation -Observation $firstRunObservation -Instances $instances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstReturned.InstanceGuid)
        return $instances.Count -eq 1 -and [System.IO.File]::Exists($evidencePath)
    }
    $firstInstances = @(Get-TaskInstances -Task $ownedTask)
    if ($firstInstances.Count -ne 1) {
        $null = Invoke-RunAttemptObservation -Observation $firstRunObservation -Instances $firstInstances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstReturned.InstanceGuid) -Final
    }
    $firstInstance = $firstInstances[0]
    $record = Get-Content -LiteralPath $evidencePath -Raw | ConvertFrom-Json
    $actionPid = [int]$record.pid
    $enginePid = [int]$firstInstance.EnginePID
    if (-not $script:ObservedProcesses.ContainsKey($enginePid)) { throw "no retained process handle exists for EnginePID $enginePid" }
    $engineProcess = $script:ObservedProcesses[$enginePid]
    $engineProcessExited = [NessaWindowsProofNative]::ProcessHasExited($engineProcess)
    $pidFacts = [NessaWindowsProofNative]::ReadTokenFacts($engineProcess)
    $observedDefinition = $observedTask.Definition
    $observedTrigger = $observedDefinition.Triggers.Item(1)
    $descriptorFacts = ConvertTo-ExactDescriptor -Sddl $observedTask.GetSecurityDescriptor($securityInformation)
    $evidence = [pscustomobject][ordered]@{
        CallerSid = $caller.Sid; PrincipalSid = (Resolve-AccountSid $observedDefinition.Principal.UserId); TriggerSid = (Resolve-AccountSid $observedTrigger.UserId)
        ActionSid = [string]$record.sid; PidBoundSid = $pidFacts.Sid
        CallerElevated = $caller.Elevated; ActionElevated = [bool]$record.elevated; PidBoundElevated = $pidFacts.Elevated
        CallerElevationType = $caller.ElevationType; ActionElevationType = [int]$record.elevationType; PidBoundElevationType = $pidFacts.ElevationType
        CallerIntegritySid = $caller.IntegritySid; ActionIntegritySid = [string]$record.integritySid; PidBoundIntegritySid = $pidFacts.IntegritySid
        ActionCreationTime = [long]$record.creationTime; PidBoundCreationTime = $pidFacts.CreationTime
        ActionPid = $actionPid; EnginePid = $enginePid; EngineProcessExited = $engineProcessExited
        PlannedNonce = $nonce; ActionNonce = [string]$record.nonce
        PlannedTaskPath = $taskPath; RunningTaskPath = [string]$firstInstance.Path
        PlannedActionId = $actionId; CurrentAction = [string]$firstInstance.CurrentAction
        InstanceGuid = [string]$firstInstance.InstanceGuid; InstanceCount = $firstInstances.Count; RunningState = [int]$firstInstance.State
        DefinitionMatches = $taskDefinitionMatches; DescriptorMatches = $taskDescriptorMatches
        Owner = $descriptorFacts.Owner; Group = $descriptorFacts.Group; DaclProtected = $descriptorFacts.Protected
        AceSids = [string]::Join('|', @($descriptorFacts.Aces | ForEach-Object { $_.Sid } | Sort-Object))
        AccessMasks = [string]::Join('|', @($descriptorFacts.Aces | ForEach-Object { $_.Mask } | Sort-Object))
        AceTypes = [string]::Join('|', @($descriptorFacts.Aces | ForEach-Object { $_.Type } | Sort-Object))
        AceFlags = [string]::Join('|', @($descriptorFacts.Aces | ForEach-Object { $_.Flags } | Sort-Object))
        ObjectAceCount = @($descriptorFacts.Aces | Where-Object { $_.IsObject }).Count; AceCount = $descriptorFacts.Aces.Count
    }
    $firstSnapshot = [pscustomobject]@{
        InstanceCount = $evidence.InstanceCount; Path = $evidence.RunningTaskPath; CurrentAction = $evidence.CurrentAction
        InstanceGuid = $evidence.InstanceGuid; EnginePid = $evidence.EnginePid; RunningState = $evidence.RunningState
        ProcessExited = $evidence.EngineProcessExited; CreationTime = $evidence.PidBoundCreationTime; Sid = $evidence.PidBoundSid
        Elevated = $evidence.PidBoundElevated; ElevationType = $evidence.PidBoundElevationType; IntegritySid = $evidence.PidBoundIntegritySid
    }
    $null = Invoke-RunAttemptObservation -Observation $firstRunObservation -Instances $firstInstances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstReturned.InstanceGuid) -Accepted $evidence -Snapshot $firstSnapshot -CompleteAcceptance -Final
    if ($firstReturned.InstanceGuid -ne $firstInstance.InstanceGuid) { throw 'Run return and enumerated singleton disagree on instance GUID' }
    Assert-NegativeEvidenceProbes -Accepted $evidence

    $secondReturned = $ownedTask.Run($null)
    $secondRunObservation = New-RunAttemptObservation
    Wait-Until -Description 'IgnoreNew singleton stabilization' -Condition {
        $instances = @(Get-TaskInstances -Task $ownedTask)
        $null = Invoke-RunAttemptObservation -Observation $secondRunObservation -Instances $instances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstInstance.InstanceGuid) -ExpectedPid ([int]$firstInstance.EnginePID)
        return $instances.Count -eq 1 -and $instances[0].InstanceGuid -eq $firstInstance.InstanceGuid
    }
    $secondInstances = @(Get-TaskInstances -Task $ownedTask)
    if ($secondInstances.Count -ne 1) {
        $null = Invoke-RunAttemptObservation -Observation $secondRunObservation -Instances $secondInstances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstInstance.InstanceGuid) -ExpectedPid ([int]$firstInstance.EnginePID) -Accepted $evidence -Final
    }
    $secondInstance = $secondInstances[0]
    if (-not $script:ObservedProcesses.ContainsKey([int]$secondInstance.EnginePID)) { throw 'IgnoreNew final snapshot lost its retained process handle' }
    $secondProcess = $script:ObservedProcesses[[int]$secondInstance.EnginePID]
    $secondProcessExited = [NessaWindowsProofNative]::ProcessHasExited($secondProcess)
    $secondPidFacts = [NessaWindowsProofNative]::ReadTokenFacts($secondProcess)
    $secondSnapshot = [pscustomobject]@{
        InstanceCount = $secondInstances.Count; Path = [string]$secondInstance.Path; CurrentAction = [string]$secondInstance.CurrentAction
        InstanceGuid = [string]$secondInstance.InstanceGuid; EnginePid = [int]$secondInstance.EnginePID; RunningState = [int]$secondInstance.State
        ProcessExited = $secondProcessExited; CreationTime = $secondPidFacts.CreationTime; Sid = $secondPidFacts.Sid
        Elevated = $secondPidFacts.Elevated; ElevationType = $secondPidFacts.ElevationType; IntegritySid = $secondPidFacts.IntegritySid
    }
    $null = Invoke-RunAttemptObservation -Observation $secondRunObservation -Instances $secondInstances -ExpectedPath $taskPath -ExpectedActionId $actionId -ExpectedGuid ([string]$firstInstance.InstanceGuid) -ExpectedPid ([int]$firstInstance.EnginePID) -Accepted $evidence -Snapshot $secondSnapshot -CompleteAcceptance -Final
    # Under IgnoreNew, Run's return is not the live instance (run 36227766725);
    # the enumerated singleton above is the authority, and this is evidence only.
    $secondReturnedGuid = if ($null -eq $secondReturned) { '(none)' } else { [string]$secondReturned.InstanceGuid }
    Write-Host "IgnoreNew second Run returned instance GUID $secondReturnedGuid; the one enumerated instance stayed $($firstInstance.InstanceGuid) with PID $($firstInstance.EnginePID)"
    if ([NessaWindowsProofNative]::ProcessHasExited($secondProcess)) { throw 'retained action process exited before proof completion' }
    Write-Host "Windows Task Scheduler model proved exact action PID $actionPid for $taskPath in the $CallerContext caller context"
}
catch {
    $primaryFailure = $_
}
finally {
    $runEffectsSettled = -not $runAttempted
    if (-not $folderOwned -and $null -ne $folderAcknowledgement -and $null -ne $schedulerRoot) {
        try {
            $recoveredFolder = Get-ExactFolder -RootFolder $schedulerRoot -Path $folderPath
            if ($null -ne $recoveredFolder) {
                $recoveredFolderMatches = Test-ExactDescriptor -Sddl $recoveredFolder.GetSecurityDescriptor($securityInformation) -CallerSid $callerSid
                $recoveredFolderOutcome = Resolve-CreateOutcome -Acknowledgement $folderAcknowledgement -Observation $(if ($recoveredFolderMatches) { 'matching' } else { 'contradictory' })
                if ($recoveredFolderOutcome.CleanupOwned) {
                    $folderOwned = $true
                    $ownedFolder = $recoveredFolder
                }
                elseif ($recoveredFolderOutcome.Preserve) {
                    Add-CleanupFailure 'scheduler folder remained contradictory after a lost or rejected create reply; it was preserved'
                }
            }
            else { $recoveredFolderOutcome = Resolve-CreateOutcome -Acknowledgement $folderAcknowledgement -Observation 'absent' }
            $folderOutcome = $recoveredFolderOutcome
        }
        catch {
            $folderOutcome = Resolve-CreateOutcome -Acknowledgement $folderAcknowledgement -Observation 'unobservable'
            Add-CleanupFailure "scheduler folder effect remained unobservable: $($_.Exception.Message)"
        }
    }
    if (-not $taskOwned -and $null -ne $taskAcknowledgement -and $folderOwned -and $null -ne $ownedFolder) {
        try {
            $recoveredTask = Get-ExactTask -Folder $ownedFolder -Name $taskName
            if ($null -ne $recoveredTask) {
                $recoveredDefinitionMatches = Test-TaskDefinition -Task $recoveredTask -CallerSid $callerSid -ActionId $actionId -Executable $powershell -Arguments $arguments -WorkingDirectory $runRoot -RegistrationSource $registrationSource
                $recoveredDescriptorMatches = Test-ExactDescriptor -Sddl $recoveredTask.GetSecurityDescriptor($securityInformation) -CallerSid $callerSid
                $recoveredTaskOutcome = Resolve-CreateOutcome -Acknowledgement $taskAcknowledgement -Observation $(if ($recoveredDefinitionMatches -and $recoveredDescriptorMatches) { 'matching' } else { 'contradictory' })
                if ($recoveredTaskOutcome.CleanupOwned) {
                    $taskOwned = $true
                    $ownedTask = $recoveredTask
                }
                elseif ($recoveredTaskOutcome.Preserve) {
                    Add-CleanupFailure 'scheduler task remained contradictory after a lost or rejected create reply; it was preserved'
                }
            }
            else { $recoveredTaskOutcome = Resolve-CreateOutcome -Acknowledgement $taskAcknowledgement -Observation 'absent' }
            $taskOutcome = $recoveredTaskOutcome
        }
        catch {
            $taskOutcome = Resolve-CreateOutcome -Acknowledgement $taskAcknowledgement -Observation 'unobservable'
            Add-CleanupFailure "scheduler task effect remained unobservable: $($_.Exception.Message)"
        }
    }
    if ((Test-RunCleanupRequired -TaskOwned $taskOwned -RunAttempted $runAttempted -Observation 'unobservable') -and $null -ne $ownedTask) {
        try { $null = Get-TaskInstances -Task $ownedTask }
        catch { Add-CleanupFailure "pre-stop exact-task enumeration failed: $($_.Exception.Message)" }
        $stopSettlement = Invoke-StopSettlement -StopEffect { $ownedTask.Stop(0) } -ProcessesSettled {
            Wait-Until -Description 'all retained task processes to exit' -Condition {
                foreach ($process in $script:ObservedProcesses.Values) {
                    if (-not [NessaWindowsProofNative]::ProcessHasExited($process)) { return $false }
                }
                return $true
            }
            return $true
        } -InstancesSettled {
            Wait-Until -Description 'exact owned task instance collection to become empty' -Condition {
                return @(Get-TaskInstances -Task $ownedTask).Count -eq 0
            }
            return $true
        }
        foreach ($failure in $stopSettlement.Failures) { Add-CleanupFailure $failure }
        $runEffectsSettled = $stopSettlement.Settled
    }
    foreach ($process in $script:ObservedProcesses.Values) { $process.Dispose() }
    $taskSettledBeforeDelete = Test-CreateEffectSettled -Acknowledgement $taskAcknowledgement -Outcome $taskOutcome
    $folderSettledBeforeDelete = Test-CreateEffectSettled -Acknowledgement $folderAcknowledgement -Outcome $folderOutcome
    $orderedCleanup = Invoke-DependencyOrderedCleanup -RunEffectsSettled $runEffectsSettled -TaskSettled $taskSettledBeforeDelete -TaskCleanupOwned ($taskOwned -and $null -ne $ownedFolder) -DeleteTask {
        $taskDeleteSettlement = Invoke-DeleteSettlement -Resource 'exact task' -DeleteEffect {
            $ownedFolder.DeleteTask($taskName, 0)
        } -ObservePresent {
            return $null -ne (Get-ExactTask -Folder $ownedFolder -Name $taskName)
        }
        if ($null -ne $taskDeleteSettlement.Failure) { Add-CleanupFailure $taskDeleteSettlement.Failure }
        return $taskDeleteSettlement.Settled
    } -FolderSettled $folderSettledBeforeDelete -FolderCleanupOwned ($folderOwned -and $null -ne $schedulerRoot) -DeleteFolder {
        $folderDeleteSettlement = Invoke-DeleteSettlement -Resource 'exact scheduler folder' -DeleteEffect {
            $schedulerRoot.DeleteFolder($folderName, 0)
        } -ObservePresent {
            return $null -ne (Get-ExactFolder -RootFolder $schedulerRoot -Path $folderPath)
        }
        if ($null -ne $folderDeleteSettlement.Failure) { Add-CleanupFailure $folderDeleteSettlement.Failure }
        return $folderDeleteSettlement.Settled
    } -RootCleanupOwned $rootOwned -DeleteRoot {
        $rootAlreadyAbsent = $null -eq $runRootHandle -and -not [System.IO.Directory]::Exists($runRoot)
        if ($rootAlreadyAbsent) { return $true }
        if ($null -eq $runRootHandle) {
            Add-CleanupFailure 'cleanup-owned run root has no retained authority handle; the path was preserved'
            return $false
        }
        try {
            if ([NessaWindowsProofNative]::DirectoryIsReparsePoint($runRootHandle) -or [NessaWindowsProofNative]::DirectoryIdentity($runRootHandle) -ne $runRootIdentity) {
                throw 'retained run-root handle identity changed'
            }
            foreach ($entry in [System.IO.DirectoryInfo]::new($runRoot).GetFileSystemInfos()) {
                $ownedFile = $entry.Name -eq 'task-action.ps1' -or $entry.Name -eq 'action-evidence.json' -or
                    ($runAttempted -and $entry.Name -match '^action-evidence\.json\.\d+\.tmp$')
                if (-not $ownedFile -or $entry -is [System.IO.DirectoryInfo] -or ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
                    throw "unexpected run-root entry was preserved: $($entry.Name)"
                }
                [System.IO.File]::Delete($entry.FullName)
            }
            if ([System.IO.DirectoryInfo]::new($runRoot).GetFileSystemInfos().Count -ne 0) {
                throw 'run root was not empty after exact owned-file cleanup'
            }
            [NessaWindowsProofNative]::MarkDirectoryForDeletion($runRootHandle)
            $runRootHandle.Dispose()
            if ([System.IO.Directory]::Exists($runRoot)) {
                Add-CleanupFailure 'exact run root remained after handle-bound deletion'
                return $false
            }
            return $true
        }
        catch {
            Add-CleanupFailure "exact run-root cleanup failed: $($_.Exception.Message)"
            return $false
        }
    }
    foreach ($preservation in $orderedCleanup.Preservations) { Add-CleanupFailure $preservation }
    if ($null -ne $runRootHandle -and -not $runRootHandle.IsClosed) { $runRootHandle.Dispose() }
}

if ($null -ne $primaryFailure -or $script:CleanupFailures.Count -ne 0) {
    throw (Format-ProofFailures -PrimaryFailure $primaryFailure -CleanupFailures @($script:CleanupFailures))
}
