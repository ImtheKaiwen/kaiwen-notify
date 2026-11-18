# Installs user-level hooks without deleting existing hooks or overwriting Codex notify.
# Run from PowerShell: powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-hooks.ps1
param(
    [ValidateSet('all','claude','codex','antigravity')][string]$Provider = 'all',
    # on: add the SubagentStop hook (Claude Code, Codex); off: remove it; keep: leave it as it is.
    [ValidateSet('keep','on','off')][string]$Subagent = 'keep',
    # Only manage the SubagentStop hook; do not touch the Stop hooks.
    [switch]$SubagentOnly
)
$ErrorActionPreference = 'Stop'

function Write-Utf8([string]$Path, [string]$Text) {
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) { [void](New-Item -ItemType Directory -Force -Path $parent) }
    # Atomic: write a sibling temp file, then replace the target in one step.
    $tmp = "$Path.kaiwen-tmp-$PID"
    [IO.File]::WriteAllText($tmp, $Text, (New-Object System.Text.UTF8Encoding($false)))
    try {
        if (Test-Path -LiteralPath $Path) { [IO.File]::Replace($tmp, $Path, [NullString]::Value) }
        else { [IO.File]::Move($tmp, $Path) }
    } catch { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue; throw }
}
function Backup-Existing([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        $backup = "$Path.kaiwen-backup-$(Get-Date -Format 'yyyyMMdd-HHmmss-fffffff')"
        Copy-Item -LiteralPath $Path -Destination $backup -ErrorAction Stop
        Write-Host "Yedek: $backup"
    }
}
function Read-JsonObject([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        $text = [IO.File]::ReadAllText($Path)
        if (-not [string]::IsNullOrWhiteSpace($text)) { return ($text | ConvertFrom-Json -ErrorAction Stop) }
    }
    return [pscustomobject]@{}
}
function Has-Property($Object, [string]$Name) {
    return ($null -ne $Object.PSObject.Properties[$Name])
}
function Set-Property($Object, [string]$Name, $Value) {
    if (Has-Property $Object $Name) { $Object.$Name = $Value }
    else { $Object | Add-Member -MemberType NoteProperty -Name $Name -Value $Value }
}

# ---- Antigravity hook command (single contract; keep in sync with src-tauri/src/integrations.rs) ----
# Antigravity passes the quotes of a quoted `-File "path"` argument through to PowerShell, which then fails with
# "invalid characters in path". An -EncodedCommand has no spaces or quotes at all, and the script path is
# resolved at run time from %LOCALAPPDATA%, so the identical command works for every Windows user name/path.
$AntigravityScriptText = @'
$ProgressPreference = 'SilentlyContinue'; $d = $env:LOCALAPPDATA; if (-not $d) { $d = [Environment]::GetFolderPath('LocalApplicationData') }; & (Join-Path $d 'KaiwenNotify\hooks\send-event.ps1') -Source antigravity
'@
$AntigravityCommand = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand ' +
    [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($AntigravityScriptText))
# Returns 'current' (this contract), 'compatible' (older unquoted -File form that works because the path has no
# spaces/non-ASCII characters), 'legacy' (older quoted forms that fail under Antigravity) or $null (not ours).
function Get-AntigravityHandlerKind($Handler) {
    if ($null -eq $Handler -or -not (Has-Property $Handler 'command') -or $Handler.type -ne 'command') { return $null }
    $cmd = [string]$Handler.command
    if ($cmd -ceq $AntigravityCommand) { return 'current' }
    $m = [regex]::Match($cmd, '^powershell\.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File ("?)([^"]*KaiwenNotify[\\/]hooks[\\/]send-event\.ps1)\1 -Source antigravity$')
    if (-not $m.Success) { return $null }
    if ($m.Groups[1].Value -eq '' -and $m.Groups[2].Value -cmatch '^[\x21-\x7E]+$') { return 'compatible' }
    return 'legacy'
}

# SubagentStop (Claude Code and Codex): added only on request (-Subagent on) and removed with -Subagent off or on
# uninstall. Handlers are matched by the exact command string, so other people's SubagentStop hooks are never touched.
function Set-SubagentHook($Object, [string]$Command, [bool]$Want) {
    if (-not (Has-Property $Object 'hooks') -or $null -eq $Object.hooks) {
        if (-not $Want) { return $false }
        Set-Property $Object 'hooks' ([pscustomobject]@{})
    }
    $groups = @()
    if (Has-Property $Object.hooks 'SubagentStop') { $groups = @($Object.hooks.SubagentStop | Where-Object { $null -ne $_ }) }
    $has = $false
    foreach ($g in $groups) {
        foreach ($h in @($g.hooks)) {
            if ($null -ne $h -and (Has-Property $h 'command') -and $h.command -ceq $Command -and $h.type -eq 'command') { $has = $true }
        }
    }
    if ($Want) {
        if ($has) { return $false }
        $new = [pscustomobject]@{ hooks = @([pscustomobject]@{ type = 'command'; command = $Command; timeout = 10 }) }
        Set-Property $Object.hooks 'SubagentStop' (@($groups) + @($new))
        return $true
    }
    if (-not $has) { return $false }
    $kept = @()
    foreach ($g in $groups) {
        $remaining = @(@($g.hooks) | Where-Object { -not ($null -ne $_ -and (Has-Property $_ 'command') -and $_.command -ceq $Command -and $_.type -eq 'command') })
        if ($remaining.Count -gt 0) { $g.hooks = @($remaining); $kept += $g }
        elseif (@($g.hooks).Count -eq 0) { $kept += $g }
        else {
            $extra = @($g.PSObject.Properties.Name | Where-Object { $_ -ne 'hooks' })
            if ($extra.Count -gt 0) { $g.hooks = @(); $kept += $g }
        }
    }
    if ($kept.Count -gt 0) { $Object.hooks.SubagentStop = @($kept) }
    else { [void]$Object.hooks.PSObject.Properties.Remove('SubagentStop') }
    return $true
}

$target = Join-Path $env:LOCALAPPDATA 'KaiwenNotify\hooks'
$hookScript = Join-Path $target 'send-event.ps1'
[void](New-Item -ItemType Directory -Force -Path $target)
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'send-event.ps1') -Destination $hookScript -Force
Write-Host "Hook kurulumu: $hookScript"

if ($Provider -in 'all','claude') {
if (-not $SubagentOnly) {
# 1. Claude Code: user-level Stop hook. All existing hooks remain in place.
$claudePath = Join-Path $env:USERPROFILE '.claude\settings.json'
$claude = Read-JsonObject $claudePath
if (-not (Has-Property $claude 'hooks')) { Set-Property $claude 'hooks' ([pscustomobject]@{}) }
if ($null -eq $claude.hooks) { Set-Property $claude 'hooks' ([pscustomobject]@{}) }
$claudeStop = @($claude.hooks.Stop | Where-Object { $null -ne $_ })
$already = @($claudeStop | Where-Object { (($_ | ConvertTo-Json -Depth 30 -Compress) -match 'KaiwenNotify') }).Count -gt 0
if (-not $already) {
    $cmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source claude'
    $handler = [pscustomobject]@{ hooks = @([pscustomobject]@{ type = 'command'; command = $cmd; timeout = 10 }) }
    Set-Property $claude.hooks 'Stop' (@($claudeStop) + @($handler))
    Backup-Existing $claudePath
    Write-Utf8 $claudePath ($claude | ConvertTo-Json -Depth 40)
    Write-Host 'Claude Code: etkin.'
} else { Write-Host 'Claude Code: zaten kurulu.' }
}

$claudePath = Join-Path $env:USERPROFILE '.claude\settings.json'
$claudeSubCmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source claude'
if ($Subagent -ne 'keep' -and (Test-Path -LiteralPath $claudePath)) {
    $claudeNow = Read-JsonObject $claudePath
    if (Set-SubagentHook $claudeNow $claudeSubCmd ($Subagent -eq 'on')) {
        Backup-Existing $claudePath
        Write-Utf8 $claudePath ($claudeNow | ConvertTo-Json -Depth 40)
        Write-Host "Claude Code: SubagentStop hook $Subagent."
    }
}
}

if ($Provider -in 'all','antigravity' -and -not $SubagentOnly) {
# 2. Google Antigravity (Desktop App, IDE and CLI read the same global file): named Stop hook group.
$antiPath = Join-Path $env:USERPROFILE '.gemini\config\hooks.json'
$anti = Read-JsonObject $antiPath
$newHandler = { [pscustomobject]@{ type = 'command'; command = $AntigravityCommand; timeout = 10 } }
$changed = $false
if (-not (Has-Property $anti 'kaiwen-notify')) {
    Set-Property $anti 'kaiwen-notify' ([pscustomobject]@{ enabled = $true; Stop = @(& $newHandler) })
    $changed = $true
    Write-Host 'Antigravity: etkin.'
} else {
    $group = $anti.'kaiwen-notify'
    if (-not (Has-Property $group 'Stop')) { Set-Property $group 'Stop' @() }
    $handlers = @($group.Stop | Where-Object { $null -ne $_ })
    $ours = @($handlers | Where-Object { $null -ne (Get-AntigravityHandlerKind $_) })
    $keep = @($ours | Where-Object { (Get-AntigravityHandlerKind $_) -eq 'current' }) + @($ours | Where-Object { (Get-AntigravityHandlerKind $_) -eq 'compatible' })
    if ($keep.Count -gt 0) {
        # A working handler exists; only drop duplicates/legacy leftovers of ours. Other people's handlers stay.
        if ($ours.Count -gt 1) {
            $group.Stop = @(@($handlers | Where-Object { $null -eq (Get-AntigravityHandlerKind $_) }) + @($keep[0]))
            $changed = $true
            Write-Host 'Antigravity: yinelenen eski komutlar temizlendi.'
        } else { Write-Host 'Antigravity: zaten kurulu.' }
    } elseif ($ours.Count -gt 0) {
        # Only broken legacy forms: upgrade the first in place, drop the rest.
        $ours[0].command = $AntigravityCommand
        $group.Stop = @(@($handlers | Where-Object { $null -eq (Get-AntigravityHandlerKind $_) -or [object]::ReferenceEquals($_, $ours[0]) }))
        $changed = $true
        Write-Host 'Antigravity: eski komut guncellendi.'
    } else {
        $group.Stop = @($handlers) + @(& $newHandler)
        $changed = $true
        Write-Host 'Antigravity: etkin.'
    }
}
if ($changed) {
    Backup-Existing $antiPath
    Write-Utf8 $antiPath ($anti | ConvertTo-Json -Depth 40)
}
}

if ($Provider -in 'all','codex') {
if (-not $SubagentOnly) {
# 3. Codex CLI / Desktop: install independent Stop hook in hooks.json.
# IMPORTANT: never change Codex Desktop's top-level notify; it may be the
# codex-computer-use.exe turn-ended safety/lifecycle callback.
$codexHooksPath = Join-Path $env:USERPROFILE '.codex\hooks.json'
$codexHooks = Read-JsonObject $codexHooksPath
if (-not (Has-Property $codexHooks 'hooks')) { Set-Property $codexHooks 'hooks' ([pscustomobject]@{}) }
if ($null -eq $codexHooks.hooks) { Set-Property $codexHooks 'hooks' ([pscustomobject]@{}) }
$codexStop = @($codexHooks.hooks.Stop | Where-Object { $null -ne $_ })
$cmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source codex'
$already = $false
foreach ($group in $codexStop) {
    foreach ($handler in @($group.hooks)) {
        if ($null -ne $handler -and (Has-Property $handler 'command') -and
            $handler.command -ceq $cmd -and $handler.type -eq 'command') {
            $already = $true
        }
    }
}
if (-not $already) {
    $handler = [pscustomobject]@{ hooks = @([pscustomobject]@{ type = 'command'; command = $cmd; timeout = 10 }) }
    Set-Property $codexHooks.hooks 'Stop' (@($codexStop) + @($handler))
    Backup-Existing $codexHooksPath
    Write-Utf8 $codexHooksPath ($codexHooks | ConvertTo-Json -Depth 40)
    Write-Host 'Codex: Stop hook eklendi. Mevcut config.toml ve notify korunuyor.'
    Write-Host 'Codex ilk kullanimda hook guven onayi isteyebilir (CLI: /hooks; Desktop: Settings > Hooks).'
} else { Write-Host 'Codex: Stop hook zaten kurulu; native notify korunuyor.' }
}

$codexSubPath = Join-Path $env:USERPROFILE '.codex\hooks.json'
$codexSubCmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source codex'
if ($Subagent -ne 'keep' -and (Test-Path -LiteralPath $codexSubPath)) {
    $codexNow = Read-JsonObject $codexSubPath
    if (Set-SubagentHook $codexNow $codexSubCmd ($Subagent -eq 'on')) {
        Backup-Existing $codexSubPath
        Write-Utf8 $codexSubPath ($codexNow | ConvertTo-Json -Depth 40)
        Write-Host "Codex: SubagentStop hook $Subagent. Codex hook guven onayi isteyebilir."
    }
}
}

Write-Host ''
Write-Host 'Tamamlandi. Kaiwen Notify acikken gorev bittiginde ses calacak.'
Write-Host 'Ayar degisiklikleri icin Claude/Codex/Antigravity oturumlarini yeniden baslatman gerekebilir.'
