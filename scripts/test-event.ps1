param([ValidateSet('claude','codex','antigravity')][string]$Source='claude')
$bytes = [Text.Encoding]::UTF8.GetBytes('{"source":"' + $Source + '","kind":"completed"}')
$udp = New-Object System.Net.Sockets.UdpClient
try { $udp.Connect('127.0.0.1', 47451); [void]$udp.Send($bytes, $bytes.Length) }
finally { $udp.Dispose() }
Write-Host 'Test olayi gonderildi. Kaiwen Notify aciksa ses duyulmalidir.'
