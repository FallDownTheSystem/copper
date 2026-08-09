<#
.SYNOPSIS
	Adds or removes one directory from the machine PATH. Invoked by Copper's
	NSIS installer hooks (windows/hooks.nsh), never by the app.

.DESCRIPTION
	The read-modify-write happens here rather than in NSIS because the NSIS
	build Tauri bundles reports NSIS_MAX_STRLEN=1024 (checked with
	`makensis -HDRINFO`). A machine PATH routinely exceeds 1023 characters, so
	`ReadRegStr` would silently truncate the value, and writing that truncated
	value back would destroy the tail of the user's PATH.

	Three properties matter more than brevity here, because the machine PATH is
	global state no other installer will repair for us:

	- The value is read WITHOUT expanding environment references. A machine PATH
	  is REG_EXPAND_SZ and normally holds entries like %SystemRoot%\system32.
	  Reading it through an expanding API and writing the result back would
	  replace every such reference with today's literal path, permanently.
	- The original RegistryValueKind is written back, so REG_EXPAND_SZ does not
	  silently become REG_SZ, and every segment this script did not come to
	  change is preserved byte-identical, empty segments included.
	- When the result equals what is already stored, nothing is written at all.

	Exit code 0 means the PATH is in the requested state. Any other code means
	it is not, and the calling hook reports that to the installer's detail log.

.PARAMETER Action
	Add or Remove. Both are idempotent: Add leaves exactly one entry however
	many times it runs, and Remove on an absent entry is a successful no-op.

.PARAMETER Dir
	The directory to add or remove, e.g. C:\Program Files\Copper\cli.

.PARAMETER TestKey
	Test seam. When supplied, the script reads and writes HKCU\<TestKey>
	instead of the machine environment key, and skips the broadcast. The
	installer hooks never pass it. It exists so the add/remove logic can be
	exercised without touching a real PATH. It is validated as non-empty, so a
	caller that passes it by mistake with an empty value fails loudly instead of
	silently falling through to the real machine PATH.
#>
[CmdletBinding()]
param(
	[Parameter(Mandatory)] [ValidateSet("Add", "Remove")] [string]$Action,
	[Parameter(Mandatory)] [string]$Dir,
	[ValidateNotNullOrEmpty()] [string]$TestKey
)

$ErrorActionPreference = "Stop"
$useTestKey = $PSBoundParameters.ContainsKey("TestKey")

# PATH has no escape for its own separator. A directory containing one cannot be
# represented as a single segment, and inserting it would add two bogus segments
# that Remove could never match again — so refuse before touching anything. NSIS
# lets the user choose the install directory, which is how such a path could
# reach us at all.
if ($Dir.Contains(";")) {
	[Console]::Error.WriteLine("copper-path: refusing to edit PATH. The directory contains a semicolon, which PATH uses as its separator: $Dir")
	exit 2
}

# Comparison only. The return value is never written back, so it is safe for it
# to be lossy: it tolerates the spellings a PATH segment picks up over the
# years — surrounding spaces, surrounding quotes, a trailing separator — while
# the segment that gets written stays exactly as it was found.
function Get-Comparable([string]$segment) {
	$text = $segment.Trim().Trim('"')
	while ($text.Length -gt 1 -and $text.EndsWith("\")) {
		$text = $text.Substring(0, $text.Length - 1)
	}
	return $text
}

$key = $null
try {
	if ($useTestKey) {
		$key = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
			[Microsoft.Win32.RegistryHive]::CurrentUser,
			[Microsoft.Win32.RegistryView]::Registry64).CreateSubKey($TestKey)
	}
	else {
		# Registry64 is named rather than left to the process: the NSIS
		# installer is a 32-bit process, and an explicit view settles any
		# question of WOW64 redirection instead of relying on this key not
		# being one of the redirected ones.
		$key = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
			[Microsoft.Win32.RegistryHive]::LocalMachine,
			[Microsoft.Win32.RegistryView]::Registry64).OpenSubKey(
			"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", $true)
	}
	if ($null -eq $key) {
		throw "Could not open the environment registry key for writing (administrator rights are required)."
	}

	$current = $key.GetValue("Path", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
	$kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
	if ($null -eq $current) {
		$current = ""
	}
	else {
		$kind = $key.GetValueKind("Path")
		if ($current -isnot [string]) {
			throw "Path is stored as $kind, not as a string. Refusing to rewrite it."
		}
	}

	$segments = [System.Collections.Generic.List[string]]::new()
	if ($current -ne "") {
		foreach ($segment in ($current -split ";")) { [void]$segments.Add($segment) }
	}

	# Exact match on split segments, never a substring search: this is what
	# keeps ...\Copper\cli from matching or stripping a ...\Copper\cli2.
	#
	# OrdinalIgnoreCase rather than PowerShell's -eq, which compares strings
	# linguistically: under -eq a composed "é" equals a decomposed "e" + U+0301,
	# and NTFS treats those as two different directories. Case-insensitive
	# ordinal is what Windows itself uses to compare paths.
	$wanted = Get-Comparable $Dir
	for ($i = $segments.Count - 1; $i -ge 0; $i--) {
		if ([string]::Equals((Get-Comparable $segments[$i]), $wanted, [System.StringComparison]::OrdinalIgnoreCase)) {
			$segments.RemoveAt($i)
		}
	}

	if ($Action -eq "Add") {
		# Insert after the last non-empty segment, so a PATH that already ends
		# in ';' keeps its trailing separator instead of gaining an empty entry
		# in the middle.
		$at = $segments.Count
		while ($at -gt 0 -and $segments[$at - 1] -eq "") { $at-- }
		$segments.Insert($at, $Dir)
	}

	$updated = [string]::Join(";", $segments)
	if ([string]::Equals($updated, $current, [System.StringComparison]::Ordinal)) {
		Write-Output "copper-path: machine PATH already correct for $Action, left untouched."
	}
	else {
		$key.SetValue("Path", $updated, $kind)
		Write-Output "copper-path: $Action $Dir (PATH stored as $kind)."
	}
}
catch {
	[Console]::Error.WriteLine("copper-path: $($_.Exception.Message)")
	exit 1
}
finally {
	if ($null -ne $key) { $key.Dispose() }
}

# Broadcast WM_SETTINGCHANGE so a *new* process — a freshly opened terminal —
# resolves the change without a reboot or a logoff. Already-running shells
# cached their environment at launch and are unaffected either way, by design.
#
# A failure here is reported but does not fail the script: the PATH is already
# written by this point, and the only consequence is that new processes wait
# for the next logon instead of picking it up now.
if (-not $useTestKey) {
	try {
		Add-Type -Namespace Copper -Name NativeMethods -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
		$HWND_BROADCAST = [IntPtr]0xffff
		$WM_SETTINGCHANGE = 0x1A
		$SMTO_ABORTIFHUNG = 0x2
		$result = [UIntPtr]::Zero
		# An ordinary failure returns zero rather than throwing, so the return
		# value is the only signal there is — without this check the catch below
		# would never fire for the most likely failure.
		$sent = [Copper.NativeMethods]::SendMessageTimeout(
			$HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, "Environment",
			$SMTO_ABORTIFHUNG, 5000, [ref]$result)
		if ($sent -eq [IntPtr]::Zero) {
			[Console]::Error.WriteLine("copper-path: WM_SETTINGCHANGE broadcast did not complete (Win32 error $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())). A new terminal may not see the change until the next sign-in.")
		}
	}
	catch {
		[Console]::Error.WriteLine("copper-path: WM_SETTINGCHANGE broadcast failed: $($_.Exception.Message)")
	}
}

exit 0
