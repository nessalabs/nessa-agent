[CmdletBinding()]
param()

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
    private const int TokenUser = 1;
    private const int TokenElevationType = 18;
    private const int TokenElevation = 20;
    private const int TokenIntegrityLevel = 25;
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

    private static string SidText(IntPtr sid)
    {
        IntPtr text;
        if (!ConvertSidToStringSidW(sid, out text))
            throw new Win32Exception(Marshal.GetLastWin32Error());
        try { return Marshal.PtrToStringUni(text); }
        finally { LocalFree(text); }
    }

    private static byte[] TokenInformation(IntPtr token, int informationClass)
    {
        int needed;
        GetTokenInformation(token, informationClass, IntPtr.Zero, 0, out needed);
        int error = Marshal.GetLastWin32Error();
        if (needed <= 0 || error != 122) throw new Win32Exception(error);
        var bytes = new byte[needed];
        var pinned = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        try
        {
            if (!GetTokenInformation(token, informationClass, pinned.AddrOfPinnedObject(), bytes.Length, out needed))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return bytes;
        }
        finally { pinned.Free(); }
    }

    private static string TokenSid(IntPtr token, int informationClass)
    {
        var bytes = TokenInformation(token, informationClass);
        var pinned = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        try
        {
            var value = (SID_AND_ATTRIBUTES)Marshal.PtrToStructure(
                pinned.AddrOfPinnedObject(), typeof(SID_AND_ATTRIBUTES));
            return SidText(value.Sid);
        }
        finally { pinned.Free(); }
    }

    public static SafeProcessHandle OpenProcessForObservation(uint processId)
    {
        var process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, processId);
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
                throw new Win32Exception(Marshal.GetLastWin32Error());
            return new NessaTokenFacts {
                Sid = TokenSid(token, TokenUser),
                Elevated = BitConverter.ToInt32(TokenInformation(token, TokenElevation), 0) != 0,
                ElevationType = BitConverter.ToInt32(TokenInformation(token, TokenElevationType), 0),
                IntegritySid = TokenSid(token, TokenIntegrityLevel),
                CreationTime = ProcessCreationTime(process)
            };
        }
        finally
        {
            if (token != IntPtr.Zero) CloseHandle(token);
        }
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

function Invoke-StopSettlement {
    param(
        [Parameter(Mandatory)] [scriptblock] $StopEffect,
        [Parameter(Mandatory)] [scriptblock] $ProcessesSettled,
        [Parameter(Mandatory)] [scriptblock] $InstancesSettled
    )
    $failures = [System.Collections.Generic.List[string]]::new()
    $stopDiagnostic = $null
    try { $null = & $StopEffect }
    catch { $stopDiagnostic = $_.Exception.Message }
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
    return [pscustomobject]@{ StopCalled = $true; Failures = @($failures) }
}

function Invoke-DeleteSettlement {
    param(
        [Parameter(Mandatory)] [string] $Resource,
        [Parameter(Mandatory)] [scriptblock] $DeleteEffect,
        [Parameter(Mandatory)] [scriptblock] $ObservePresent
    )
    $deleteDiagnostic = $null
    try { $null = & $DeleteEffect }
    catch { $deleteDiagnostic = $_.Exception.Message }
    try {
        if (& $ObservePresent) {
            $detail = if ($null -ne $deleteDiagnostic) { ": $deleteDiagnostic" } else { '' }
            return "$Resource remained after deletion$detail"
        }
        return $null
    }
    catch { return "$Resource absence could not be observed: $($_.Exception.Message)" }
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
    $lostStop = Invoke-StopSettlement -StopEffect { throw 'lost stop reply' } -ProcessesSettled { $true } -InstancesSettled { $true }
    if (-not $lostStop.StopCalled -or $lostStop.Failures.Count -ne 0) { throw 'lost stop reply did not settle through fresh observations' }
    $failedStop = Invoke-StopSettlement -StopEffect { throw 'stop rejected' } -ProcessesSettled { $false } -InstancesSettled { $false }
    if ($failedStop.Failures.Count -ne 2 -or $failedStop.Failures[0] -notmatch 'stop rejected') { throw 'stop failure and lingering effects were not retained' }
    $lostDelete = Invoke-DeleteSettlement -Resource 'task' -DeleteEffect { throw 'lost delete reply' } -ObservePresent { $false }
    if ($null -ne $lostDelete) { throw 'lost delete reply did not settle through confirmed absence' }
    $failedDelete = Invoke-DeleteSettlement -Resource 'task' -DeleteEffect { throw 'delete rejected' } -ObservePresent { $true }
    if ($failedDelete -notmatch 'task remained.*delete rejected') { throw 'delete rejection and retained object were not retained together' }
    $unobservableDelete = Invoke-DeleteSettlement -Resource 'folder' -DeleteEffect { } -ObservePresent { throw 'observation failed' }
    if ($unobservableDelete -notmatch 'absence could not be observed') { throw 'unobservable cleanup did not remain a failure' }
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
    $definition = $Task.Definition
    if ($definition.Principal.UserId -ne $CallerSid -or $definition.Principal.LogonType -ne 3 -or $definition.Principal.RunLevel -ne 0) { return $false }
    if ($definition.Triggers.Count -ne 1) { return $false }
    $trigger = $definition.Triggers.Item(1)
    if ($trigger.Type -ne 9 -or -not $trigger.Enabled -or $trigger.UserId -ne $CallerSid) { return $false }
    if ($definition.Actions.Count -ne 1) { return $false }
    $action = $definition.Actions.Item(1)
    if ($action.Type -ne 0 -or $action.Id -ne $ActionId -or $action.Path -ne $Executable -or $action.Arguments -ne $Arguments -or $action.WorkingDirectory -ne $WorkingDirectory) { return $false }
    $settings = $definition.Settings
    if ($settings.MultipleInstances -ne 2 -or -not $settings.AllowDemandStart -or $settings.ExecutionTimeLimit -ne 'PT0S') { return $false }
    if ($settings.DisallowStartIfOnBatteries -or $settings.StopIfGoingOnBatteries -or $settings.RunOnlyIfIdle -or $settings.RunOnlyIfNetworkAvailable) { return $false }
    if ($settings.RestartCount -ne 3 -or $settings.RestartInterval -ne 'PT1M') { return $false }
    if ($definition.RegistrationInfo.Source -ne $RegistrationSource) { return $false }
    return $true
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
        if (Test-CompleteAgreement -Evidence ([pscustomobject]$copy)) {
            throw "evidence validator accepted mutation of $($entry.Key)"
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
$callerHandle = [NessaWindowsProofNative]::OpenProcessForObservation([uint32]$PID)
try { $caller = [NessaWindowsProofNative]::ReadTokenFacts($callerHandle) }
finally { $callerHandle.Dispose() }
if ($caller.Elevated) { throw 'the Windows runner process is elevated; the least-privilege current-user model is not proved' }

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
    if (-not $taskDefinitionMatches) { throw 'registered task definition contradicts the plan' }
    if (-not $taskDescriptorMatches) { throw 'registered task descriptor contradicts the plan' }
    if ($observedTask.Xml -match '(?i)password|credential|secret') { throw 'registered task XML contains a credential-shaped field' }

    $runAttempted = $true
    $firstReturned = $ownedTask.Run($null)
    Wait-Until -Description 'one running task instance and action evidence' -Condition {
        $instances = @(Get-TaskInstances -Task $ownedTask)
        return $instances.Count -eq 1 -and [System.IO.File]::Exists($evidencePath)
    }
    $firstInstances = @(Get-TaskInstances -Task $ownedTask)
    if ($firstInstances.Count -ne 1) { throw "first run exposed $($firstInstances.Count) instances" }
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
        CallerSid = $caller.Sid; PrincipalSid = $observedDefinition.Principal.UserId; TriggerSid = $observedTrigger.UserId
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
    if (-not (Test-CompleteAgreement -Evidence $evidence)) { throw 'first running instance did not satisfy the complete identity, token, and ACL vector' }
    if ($firstReturned.InstanceGuid -ne $firstInstance.InstanceGuid) { throw 'Run return and enumerated singleton disagree on instance GUID' }
    Assert-NegativeEvidenceProbes -Accepted $evidence

    $secondReturned = $ownedTask.Run($null)
    Wait-Until -Description 'IgnoreNew singleton stabilization' -Condition {
        $instances = @(Get-TaskInstances -Task $ownedTask)
        return $instances.Count -eq 1 -and $instances[0].InstanceGuid -eq $firstInstance.InstanceGuid
    }
    $secondInstances = @(Get-TaskInstances -Task $ownedTask)
    if ($secondInstances.Count -ne 1 -or $secondInstances[0].InstanceGuid -ne $firstInstance.InstanceGuid -or $secondInstances[0].EnginePID -ne $firstInstance.EnginePID) {
        throw 'IgnoreNew did not retain the original exact singleton'
    }
    if ($secondReturned.InstanceGuid -ne $firstInstance.InstanceGuid) { throw 'second Run did not return the original IgnoreNew instance' }
    Write-Host "Windows Task Scheduler model proved exact action PID $actionPid for $taskPath"
}
catch {
    $primaryFailure = $_
}
finally {
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
        }
        catch { Add-CleanupFailure "scheduler folder effect remained unobservable: $($_.Exception.Message)" }
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
        }
        catch { Add-CleanupFailure "scheduler task effect remained unobservable: $($_.Exception.Message)" }
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
    }
    foreach ($process in $script:ObservedProcesses.Values) { $process.Dispose() }
    if ($taskOwned -and $null -ne $ownedFolder) {
        $taskDeleteFailure = Invoke-DeleteSettlement -Resource 'exact task' -DeleteEffect {
            $ownedFolder.DeleteTask($taskName, 0)
        } -ObservePresent {
            return $null -ne (Get-ExactTask -Folder $ownedFolder -Name $taskName)
        }
        if ($null -ne $taskDeleteFailure) { Add-CleanupFailure $taskDeleteFailure }
    }
    if ($folderOwned -and $null -ne $schedulerRoot) {
        $folderDeleteFailure = Invoke-DeleteSettlement -Resource 'exact scheduler folder' -DeleteEffect {
            $schedulerRoot.DeleteFolder($folderName, 0)
        } -ObservePresent {
            return $null -ne (Get-ExactFolder -RootFolder $schedulerRoot -Path $folderPath)
        }
        if ($null -ne $folderDeleteFailure) { Add-CleanupFailure $folderDeleteFailure }
    }
    if ($rootOwned) {
        $rootAlreadyAbsent = $null -eq $runRootHandle -and -not [System.IO.Directory]::Exists($runRoot)
        if ($rootAlreadyAbsent) { $rootOwned = $false }
        $rootCanDelete = $null -ne $runRootHandle
        if ($rootOwned -and -not $rootCanDelete) { Add-CleanupFailure 'cleanup-owned run root has no retained authority handle; the path was preserved' }
        if ($rootCanDelete) {
            try {
                if ([NessaWindowsProofNative]::DirectoryIsReparsePoint($runRootHandle) -or [NessaWindowsProofNative]::DirectoryIdentity($runRootHandle) -ne $runRootIdentity) {
                    throw 'retained run-root handle identity changed'
                }
            }
            catch {
                $rootCanDelete = $false
                Add-CleanupFailure "exact run-root identity could not be reobserved through its retained handle: $($_.Exception.Message)"
            }
        }
        if ($rootCanDelete) {
            try {
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
            }
            catch {
                $rootCanDelete = $false
                Add-CleanupFailure "exact run-root cleanup failed: $($_.Exception.Message)"
            }
        }
        if ($null -ne $runRootHandle) { $runRootHandle.Dispose() }
        if ($rootCanDelete -and [System.IO.Directory]::Exists($runRoot)) {
            Add-CleanupFailure 'exact run root remained after handle-bound deletion'
        }
    }
}

if ($null -ne $primaryFailure -or $script:CleanupFailures.Count -ne 0) {
    throw (Format-ProofFailures -PrimaryFailure $primaryFailure -CleanupFailures @($script:CleanupFailures))
}
