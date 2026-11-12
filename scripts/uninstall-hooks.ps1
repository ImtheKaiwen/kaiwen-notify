# Remove only the Kaiwen Notify hooks installed by install-hooks.ps1.
# Existing third-party hooks, later config changes, helper files and backups are preserved.
# Preview: powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\uninstall-hooks.ps1 -DryRun
# Apply:   powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\uninstall-hooks.ps1
param([switch]$DryRun, [ValidateSet('all','claude','codex','antigravity')][string]$Provider = 'all')
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
        Write-Host "Geri alma oncesi yedek: $backup"
    }
}
function Read-JsonObject([string]$Path) {
    $contents = [IO.File]::ReadAllText($Path)
    if ([string]::IsNullOrWhiteSpace($contents)) { return [pscustomobject]@{} }
    return ($contents | ConvertFrom-Json -ErrorAction Stop)
}
function Has-Property($Object, [string]$Name) {
    return ($null -ne $Object -and $null -ne $Object.PSObject.Properties[$Name])
}
function Commit-Json([string]$Path, $Object, [string]$Description) {
    if ($DryRun) { Write-Host "[DRY RUN] $Description"; return }
    Backup-Existing $Path
    Write-Utf8 $Path ($Object | ConvertTo-Json -Depth 40)
    Write-Host $Description
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

$hookScript = Join-Path $env:LOCALAPPDATA 'KaiwenNotify\hooks\send-event.ps1'
$claudeExpected = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source claude'

if ($Provider -in 'all','claude') {
# Claude Code: remove only the exact command handler from Stop groups.
$claudePath = Join-Path $env:USERPROFILE '.claude\settings.json'
if (Test-Path -LiteralPath $claudePath) {
    $claude = Read-JsonObject $claudePath
    if ((Has-Property $claude 'hooks') -and (Has-Property $claude.hooks 'Stop')) {
        $newGroups = @()
        $removed = 0
        foreach ($group in @($claude.hooks.Stop)) {
            if ($null -eq $group -or -not (Has-Property $group 'hooks')) {
                $newGroups += $group
                continue
            }
            $remainingHandlers = @()
            foreach ($handler in @($group.hooks)) {
                if ($null -ne $handler -and (Has-Property $handler 'command') -and
                    $handler.command -ceq $claudeExpected -and $handler.type -eq 'command') {
                    $removed++
                } else {
                    $remainingHandlers += $handler
                }
            }
            if ($remainingHandlers.Count -gt 0) {
                $group.hooks = @($remainingHandlers)
                $newGroups += $group
            } elseif (@($group.hooks).Count -eq 0) {
                # Keep unrelated empty groups as-is.
                $newGroups += $group
            } else {
                # The installer created a group containing only this handler.
                # If extra group metadata was subsequently added, preserve it.
                $extraFields = @($group.PSObject.Properties.Name | Where-Object { $_ -ne 'hooks' })
                if ($extraFields.Count -gt 0) {
                    $group.hooks = @()
                    $newGroups += $group
                }
            }
        }
        if ($removed -gt 0) {
            if ($newGroups.Count -gt 0) { $claude.hooks.Stop = @($newGroups) }
            else { [void]$claude.hooks.PSObject.Properties.Remove('Stop') }
            Commit-Json $claudePath $claude "Claude Code: $removed Kaiwen Notify hook kaldirildi."
        } else { Write-Host 'Claude Code: kaldirilacak Kaiwen Notify hook bulunamadi.' }
    }
}
}

if ($Provider -in 'all','antigravity') {
# Antigravity: remove only our own handlers (every known command form) from our named group.
$antiPath = Join-Path $env:USERPROFILE '.gemini\config\hooks.json'
if (Test-Path -LiteralPath $antiPath) {
    $anti = Read-JsonObject $antiPath
    if (Has-Property $anti 'kaiwen-notify') {
        $group = $anti.'kaiwen-notify'
        if (Has-Property $group 'Stop') {
            $originalHandlers = @($group.Stop | Where-Object { $null -ne $_ })
            $others = @($originalHandlers | Where-Object {
                $null -eq (Get-AntigravityHandlerKind $_)
            })
            $removed = $originalHandlers.Count - $others.Count
            if ($removed -gt 0) {
                $extraFields = @($group.PSObject.Properties.Name | Where-Object { $_ -notin @('Stop', 'enabled') })
                if ($others.Count -eq 0 -and $extraFields.Count -eq 0) {
                    [void]$anti.PSObject.Properties.Remove('kaiwen-notify')
                } elseif ($others.Count -eq 0) {
                    [void]$group.PSObject.Properties.Remove('Stop')
                } else { $group.Stop = @($others) }
                Commit-Json $antiPath $anti "Antigravity: $removed Kaiwen Notify hook kaldirildi."
            } else { Write-Host 'Antigravity: eslesen Kaiwen Notify hook bulunamadi; ayarlar korunuyor.' }
        }
    }
}
}

if ($Provider -in 'all','codex') {
# Codex CLI / Desktop: only remove our own Stop handler from hooks.json.
# Do not touch the Codex Desktop computer-use notify field.
$codexHooksPath = Join-Path $env:USERPROFILE '.codex\hooks.json'
$codexExpected = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $hookScript + '" -Source codex'
if (Test-Path -LiteralPath $codexHooksPath) {
    $codexHooks = Read-JsonObject $codexHooksPath
    if ((Has-Property $codexHooks 'hooks') -and (Has-Property $codexHooks.hooks 'Stop')) {
        $newGroups = @()
        $removed = 0
        foreach ($group in @($codexHooks.hooks.Stop)) {
            if ($null -eq $group -or -not (Has-Property $group 'hooks')) {
                $newGroups += $group
                continue
            }
            $remainingHandlers = @()
            foreach ($handler in @($group.hooks)) {
                if ($null -ne $handler -and (Has-Property $handler 'command') -and
                    $handler.command -ceq $codexExpected -and $handler.type -eq 'command') {
                    $removed++
                } else { $remainingHandlers += $handler }
            }
            if ($remainingHandlers.Count -gt 0) {
                $group.hooks = @($remainingHandlers)
                $newGroups += $group
            } elseif (@($group.hooks).Count -eq 0) {
                $newGroups += $group
            } else {
                $extraFields = @($group.PSObject.Properties.Name | Where-Object { $_ -ne 'hooks' })
                if ($extraFields.Count -gt 0) {
                    $group.hooks = @()
                    $newGroups += $group
                }
            }
        }
        if ($removed -gt 0) {
            if ($newGroups.Count -gt 0) { $codexHooks.hooks.Stop = @($newGroups) }
            else { [void]$codexHooks.hooks.PSObject.Properties.Remove('Stop') }
            Commit-Json $codexHooksPath $codexHooks "Codex: $removed Kaiwen Notify Stop hook kaldirildi. Native notify korunuyor."
        } else { Write-Host 'Codex: kaldirilacak Kaiwen Notify Stop hook bulunamadi.' }
    }
}

# Backward compatibility: remove only the exact legacy v0.2 notify field if
# Kaiwen Notify itself installed it. All native (computer-use) notify lines stay.
# Codex CLI: remove only the exact top-level notify line created by the installer.
$codexPath = Join-Path $env:USERPROFILE '.codex\config.toml'
if (Test-Path -LiteralPath $codexPath) {
    $codex = [IO.File]::ReadAllText($codexPath)
    $args = @('powershell.exe', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $hookScript, '-Source', 'codex')
    $tomlArray = ($args | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress }) -join ', '
    $expectedLine = 'notify = [' + $tomlArray + ']'
    $pattern = '(?m)^' + [regex]::Escape($expectedLine) + '(?:\r?\n|$)'
    $updated = [regex]::new($pattern).Replace($codex, '', 1)
    if ($updated -cne $codex) {
        if ($DryRun) { Write-Host '[DRY RUN] Codex CLI: Kaiwen Notify notify satiri kaldirilacak.' }
        else {
            Backup-Existing $codexPath
            Write-Utf8 $codexPath $updated
            Write-Host 'Codex CLI: Kaiwen Notify notify satiri kaldirildi.'
        }
    } else { Write-Host 'Codex CLI: eslesen Kaiwen Notify notify satiri yok; ayarlar korunuyor.' }
}
}

Write-Host ''
if ($DryRun) {
    Write-Host 'Sadece onizleme yapildi; hicbir dosya degistirilmedi.'
} else {
    Write-Host 'Hook geri alma tamamlandi. Eski oturumlari yeniden baslatin.'
    Write-Host "Yedekler ve yardimci dosya korunuyor: $hookScript"
    Write-Host 'Uygulamayi da kaldirmak icin tray menusu > Tamamen cik; sonra Windows Uygulamalar menusu.'
}
