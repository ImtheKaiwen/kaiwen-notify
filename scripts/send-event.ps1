# Kaiwen Notify: universal, best-effort hook adapter (PowerShell 5.1+)
# The hook is intentionally silent on failures and does not modify agent decisions.
# It forwards only a short whitelist of event metadata (hook name, session/turn/agent ids, agent type) to the app;
# the app classifies the event. Prompts, replies, file paths and transcripts are never read into the message.
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
            $log = Join-Path $dir 'diag.log'
            # Keep the opt-in log bounded: roll over at 256 KB.
            if ((Test-Path -LiteralPath $log) -and (Get-Item -LiteralPath $log).Length -gt 262144) {
                Move-Item -LiteralPath $log -Destination ($log + '.1') -Force
            }
            Add-Content -LiteralPath $log -Value ("{0} {1} {2}" -f (Get-Date -Format o), $Source, $Text)
        }
    } catch { }
}

function Send-Udp([string]$Message) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Message)
    # The listener port is fixed; the env override exists only so tests can run beside a live app.
    $port = 47451
    if ($env:KAIWEN_NOTIFY_PORT -match '^[0-9]{4,5}$') { $port = [int]$env:KAIWEN_NOTIFY_PORT }
    $udp = New-Object System.Net.Sockets.UdpClient
    try {
        $udp.Connect('127.0.0.1', $port)
        [void]$udp.Send($bytes, $bytes.Length)
    }
    finally { $udp.Dispose() }
}

# Trims, strips control characters and caps the length of an id-like value; $null when nothing is left.
function Get-Short($Value) {
    if ($null -eq $Value) { return $null }
    $text = ([string]$Value) -replace '[\x00-\x1f\x7f]', ''
    $text = $text.Trim()
    if ($text.Length -gt 100) { $text = $text.Substring(0, 100) }
    if ($text -eq '') { return $null }
    return $text
}

# Builds the compact JSON datagram. Absent values are omitted, so a bare event is exactly
# {"source":"claude","kind":"completed"}, which older app versions understand too.
function New-Message([string]$Kind, $Extra) {
    $message = [ordered]@{ source = $Source; kind = $Kind }
    foreach ($key in @('note', 'product', 'hook', 'session', 'turn', 'agent', 'agent_type')) {
        if ($null -ne $Extra -and $Extra.Contains($key) -and $null -ne $Extra[$key]) { $message[$key] = $Extra[$key] }
    }
    return ($message | ConvertTo-Json -Compress)
}

# Antigravity ships as three runtimes that share one hooks.json but keep separate data folders.
# The Stop payload's transcriptPath reveals which one is calling: ~/.gemini/antigravity (Desktop App),
# antigravity-ide (IDE) or antigravity-cli (CLI). Only the product name is reported, never the path.
function Get-AntigravityProduct($Payload) {
    $path = ([string]$Payload.transcriptPath).Replace('\', '/').ToLowerInvariant()
    if ($path -match '/\.gemini/antigravity-ide/') { return 'ide' }
    if ($path -match '/\.gemini/antigravity-cli/') { return 'cli' }
    if ($path -match '/\.gemini/antigravity/') { return 'app' }
    return $null
}

Write-Diag 'invoked'
try {
    $extra = [ordered]@{}
    # Claude, Codex Stop/SubagentStop hooks and Antigravity supply JSON via stdin.
    # Codex requires valid JSON on stdout, even when the app is unavailable.
    if ([Console]::IsInputRedirected) {
        $raw = [Console]::In.ReadToEnd()
        if (-not [string]::IsNullOrWhiteSpace($raw)) {
            $payload = try { $raw | ConvertFrom-Json } catch { Write-Diag 'payload not JSON'; $null }
            if ($Source -eq 'antigravity') {
                $extra['product'] = Get-AntigravityProduct $payload
                $extra['session'] = Get-Short $payload.conversationId
                $extra['turn'] = Get-Short $payload.executionNum
            }
            else {
                $extra['hook'] = Get-Short $payload.hook_event_name
                $extra['session'] = Get-Short $payload.session_id
                # Claude: prompt_id identifies the user prompt; Codex: turn_id identifies the turn.
                $extra['turn'] = if ($payload.turn_id) { Get-Short $payload.turn_id } else { Get-Short $payload.prompt_id }
                $extra['agent'] = Get-Short $payload.agent_id
                $extra['agent_type'] = Get-Short $payload.agent_type
            }
            Write-Diag ("payload hook=$($extra['hook']) product=$($extra['product']) fullyIdle=$($payload.fullyIdle) reason=$($payload.terminationReason) exec=$($payload.executionNum) agent_type=$($extra['agent_type']) bytes=$($raw.Length) keys=$(@($payload.PSObject.Properties.Name) -join ',')")
            if ($Source -eq 'antigravity') {
                # Per the Stop contract fullyIdle=false means background tasks are still running.
                if ($payload.fullyIdle -eq $false) {
                    Write-Diag 'skip: fullyIdle=false'
                    $extra['note'] = 'not_idle'
                    Send-Udp (New-Message 'seen' $extra)
                    return
                }
                # Documented reasons are examples, not an exhaustive list, so skip only known non-completions
                # instead of requiring one exact success string.
                $reason = [string]$payload.terminationReason
                if ($reason -in 'error', 'max_steps_exceeded', 'cancelled', 'canceled', 'user_cancelled') {
                    Write-Diag "skip: reason=$reason"
                    $extra['note'] = 'stopped_early'
                    Send-Udp (New-Message 'seen' $extra)
                    return
                }
            }
            if ($Source -eq 'claude' -and $payload.stop_hook_active -eq $true) { return }
        }
        else { Write-Diag 'empty stdin' }
    }
    else { Write-Diag 'stdin not redirected' }

    Write-Diag 'sending'
    Send-Udp (New-Message 'completed' $extra)
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
