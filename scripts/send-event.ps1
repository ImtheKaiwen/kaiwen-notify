# Kaiwen Notify: universal, best-effort hook adapter (PowerShell 5.1+)
# The hook is intentionally silent on failures and does not modify agent decisions.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('claude', 'codex', 'antigravity')]
    [string]$Source
)

$ErrorActionPreference = 'Stop'

# Optional diagnostics: only when %LOCALAPPDATA%\KaiwenNotify\diag.enabled exists.
# Logs event metadata only (never prompts, paths or payload text).
function Write-Diag([string]$Text) {
    try {
        $dir = Join-Path $env:LOCALAPPDATA 'KaiwenNotify'
        if (Test-Path -LiteralPath (Join-Path $dir 'diag.enabled')) {
            Add-Content -LiteralPath (Join-Path $dir 'diag.log') -Value ("{0} {1} {2}" -f (Get-Date -Format o), $Source, $Text)
        }
    } catch { }
}
Write-Diag 'invoked'
try {
    # Claude, Codex Stop hooks and Antigravity supply JSON via stdin.
    # Codex requires valid JSON on stdout, even when the app is unavailable.
    if ([Console]::IsInputRedirected) {
        $raw = [Console]::In.ReadToEnd()
        if (-not [string]::IsNullOrWhiteSpace($raw)) {
            $payload = try { $raw | ConvertFrom-Json } catch { Write-Diag 'payload not JSON'; $null }
            Write-Diag ("payload fullyIdle=$($payload.fullyIdle) reason=$($payload.terminationReason) exec=$($payload.executionNum) bytes=$($raw.Length) keys=$(@($payload.PSObject.Properties.Name) -join ',')")
            if ($Source -eq 'antigravity') {
                # Per the Stop contract fullyIdle=false means background tasks are still running.
                if ($payload.fullyIdle -eq $false) { Write-Diag 'skip: fullyIdle=false'; return }
                # Documented reasons are examples, not an exhaustive list, so skip only known non-completions
                # instead of requiring one exact success string.
                $reason = [string]$payload.terminationReason
                if ($reason -in 'error', 'max_steps_exceeded', 'cancelled', 'canceled', 'user_cancelled') { Write-Diag "skip: reason=$reason"; return }
            }
            if ($Source -eq 'claude' -and $payload.stop_hook_active -eq $true) { return }
        }
        else { Write-Diag 'empty stdin' }
    }
    else { Write-Diag 'stdin not redirected' }

    Write-Diag 'sending'
    $message = '{"source":"' + $Source + '","kind":"completed"}'
    $bytes = [Text.Encoding]::UTF8.GetBytes($message)
    $udp = New-Object System.Net.Sockets.UdpClient
    try {
        $udp.Connect('127.0.0.1', 47451)
        [void]$udp.Send($bytes, $bytes.Length)
    }
    finally { $udp.Dispose() }
}
catch {
    # Never fail an agent turn because the notification app is closed.
}
finally {
    if ($Source -eq 'antigravity') {
        # Stop hooks must return a valid non-blocking JSON response.
        [Console]::Out.WriteLine('{"decision":"stop"}')
    }
    elseif ($Source -eq 'codex') {
        # Valid non-blocking Stop hook stdout even if parsing or UDP failed.
        [Console]::Out.WriteLine('{}')
    }
}
