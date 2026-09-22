param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$ChecksumFile
)

$ErrorActionPreference = "Stop"
$binaryPath = (Resolve-Path $Binary).Path
$expected = (Get-Content $ChecksumFile).Split()[0].ToLowerInvariant()
$actual = (Get-FileHash $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "Artifact checksum mismatch: expected $expected, got $actual" }

& $binaryPath --version
if ($LASTEXITCODE -ne 0) { throw "lific --version failed" }
& $binaryPath --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw "lific --help failed" }

$scratch = Join-Path ([IO.Path]::GetTempPath()) ("lific-artifact-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $scratch | Out-Null
$server = $null
$oldName = $env:LIFIC_INIT_ADMIN_NAME
$oldPassword = $env:LIFIC_INIT_ADMIN_PASSWORD
$oldApiKey = $env:LIFIC_API_KEY
try {
    # Doctor must not send a credential for the operator's real instance to
    # this newly initialized scratch server.
    $env:LIFIC_API_KEY = $null
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()

    # Explicit config also makes doctor probe this server, not a default port
    # or a developer's existing instance. The database is relative to this file.
    $config = Join-Path $scratch "lific.toml"
    @"
[server]
host = "127.0.0.1"
port = $port
public_url = "http://127.0.0.1:$port"
[database]
path = "lific.db"
[backup]
enabled = false
[auth]
required = true
allow_signup = false
"@ | Set-Content $config
    $stdout = Join-Path $scratch "server.out"
    $stderr = Join-Path $scratch "server.err"
    $env:LIFIC_INIT_ADMIN_NAME = "CI"
    $env:LIFIC_INIT_ADMIN_PASSWORD = "ci-release-password-123"
    $server = Start-Process -FilePath $binaryPath -WorkingDirectory $scratch `
        -ArgumentList @("--config", ('"{0}"' -f $config), "start", "--init-if-missing") `
        -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $ready = $false
    while ($clock.Elapsed.TotalSeconds -lt 60) {
        if ($server.HasExited) { throw "lific exited with code $($server.ExitCode)`n$(Get-Content $stderr -Raw)" }
        # A bind log from this process prevents an unrelated listener from
        # satisfying readiness if it claimed the port after our allocation.
        if ((Get-Content $stdout -Raw) -match "lific server started") {
            try {
                $health = Invoke-WebRequest "http://127.0.0.1:$port/api/health" -TimeoutSec 2
                if ($health.StatusCode -eq 200) { $ready = $true; break }
            } catch { }
        }
        Start-Sleep -Milliseconds 200
    }
    if (-not $ready -or $server.HasExited) { throw "lific did not become healthy`n$(Get-Content $stderr -Raw)" }
    if (-not (Test-Path (Join-Path $scratch "lific.db"))) { throw "lific did not create its database" }
    & $binaryPath --config $config doctor --repair | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "lific database doctor failed" }

    $page = Invoke-WebRequest "http://127.0.0.1:$port/" -TimeoutSec 5
    if ($page.StatusCode -ne 200 -or $page.Content -notmatch "<html") { throw "Embedded web UI did not respond with HTML" }
    foreach ($extension in @("js", "css")) {
        $pattern = '(?:src|href)="(/assets/[^"\s]+\.' + $extension + ')"'
        $asset = [regex]::Match($page.Content, $pattern).Groups[1].Value
        if (-not $asset) { throw "Embedded web UI did not reference a $extension asset" }
        $response = Invoke-WebRequest "http://127.0.0.1:$port$asset" -TimeoutSec 5
        $mime = $response.Headers["Content-Type"] -join ","
        $expectedMime = if ($extension -eq "js") { "(?:java|ecma)script" } else { "text/css" }
        if ($response.StatusCode -ne 200 -or $response.RawContentLength -eq 0 -or $mime -notmatch $expectedMime) {
            throw "Embedded asset $asset was empty or returned the wrong content type: $mime"
        }
    }
    Write-Host "Verified artifact checksum, startup, database, API, and embedded JavaScript/CSS."
} finally {
    if ($null -ne $server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force
        $server.WaitForExit()
    }
    $env:LIFIC_INIT_ADMIN_NAME = $oldName
    $env:LIFIC_INIT_ADMIN_PASSWORD = $oldPassword
    $env:LIFIC_API_KEY = $oldApiKey
    Remove-Item -Recurse -Force $scratch
}
