# Installs user-level hooks without deleting existing hooks or overwriting Codex notify.
# Run from PowerShell: powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-hooks.ps1
param([ValidateSet('all','claude','codex','antigravity')][string]$Provider = 'all')
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

$target = Join-Path $env:LOCALAPPDATA 'KaiwenNotify\hooks'
$hookScript = Join-Path $target 'send-event.ps1'
[void](New-Item -ItemType Directory -Force -Path $target)
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'send-event.ps1') -Destination $hookScript -Force
Write-Host "Hook kurulumu: $hookScript"

if ($Provider -in 'all','claude') {
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

if ($Provider -in 'all','antigravity') {
# 2. Google Antigravity: global, named Stop hook group.
$antiPath = Join-Path $env:USERPROFILE '.gemini\config\hooks.json'
$anti = Read-JsonObject $antiPath
# Forward slashes survive both Windows and POSIX-style command splitters (Antigravity is a Go
# program); a backslash path inside quotes can lose its separators under POSIX-style parsing.
$hookScriptFwd = $hookScript.Replace('\', '/')
$cmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScriptFwd + '" -Source antigravity'
$legacyCmd = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source antigravity'
if (-not (Has-Property $anti 'kaiwen-notify')) {
    $group = [pscustomobject]@{ enabled = $true; Stop = @([pscustomobject]@{ type = 'command'; command = $cmd; timeout = 10 }) }
    Set-Property $anti 'kaiwen-notify' $group
    Backup-Existing $antiPath
    Write-Utf8 $antiPath ($anti | ConvertTo-Json -Depth 40)
    Write-Host 'Antigravity: etkin.'
} else {
    # Migrate only our own legacy (backslash) handler; leave anything else in the group alone.
    $group = $anti.'kaiwen-notify'
    $migrated = 0
    if (Has-Property $group 'Stop') {
        foreach ($handler in @($group.Stop)) {
            if ($null -ne $handler -and (Has-Property $handler 'command') -and $handler.command -ceq $legacyCmd) {
                $handler.command = $cmd
                $migrated++
            }
        }
    }
    if ($migrated -gt 0) {
        Backup-Existing $antiPath
        Write-Utf8 $antiPath ($anti | ConvertTo-Json -Depth 40)
        Write-Host 'Antigravity: eski komut guncellendi.'
    } else { Write-Host 'Antigravity: zaten kurulu.' }
}
}

if ($Provider -in 'all','codex') {
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

Write-Host ''
Write-Host 'Tamamlandi. Kaiwen Notify acikken gorev bittiginde ses calacak.'
Write-Host 'Ayar degisiklikleri icin Claude/Codex/Antigravity oturumlarini yeniden baslatman gerekebilir.'
