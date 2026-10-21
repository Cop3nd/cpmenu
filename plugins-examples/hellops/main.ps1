# Windows PowerShell 5.1 compatible. Startup costs ~0.3-0.5 s; the host waits up to 5 s for "ready".
[Console]::InputEncoding  = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)   # a stray BOM is tolerated by the host

function Send($obj) {
    [Console]::Out.WriteLine((ConvertTo-Json -InputObject $obj -Compress -Depth 8))
    [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    try { $msg = $line | ConvertFrom-Json } catch { continue }

    switch ($msg.type) {
        'init'  { Send @{ type = 'ready'; protocol = 1 } }
        'query' {
            $who = if ($msg.text.Trim()) { $msg.text.Trim() } else { 'world' }
            $greeting = "Hello, $who!"
            Send @{
                type  = 'results'
                id    = $msg.id
                items = @(
                    @{
                        key      = 'greet'
                        title    = $greeting
                        subtitle = 'Enter copies it'
                        score    = 80
                        default  = 'copy'
                        actions  = @(
                            @{ id = 'copy'; label = 'Copy greeting'; do = @{ copy = $greeting } },
                            @{ id = 'time'; label = 'Copy current time'; stay = $false }
                        )
                    }
                )
            }
        }
        'action' {
            if ($msg.id -eq 'time') {
                Send @{ type = 'done'; req = $msg.req; do = @(@{ copy = (Get-Date -Format o) }) }
            } else {
                Send @{ type = 'done'; req = $msg.req }
            }
        }
        'shutdown' { exit 0 }
    }
}
