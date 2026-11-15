# Installs the graphersal-memory binary from a GitHub Release (Windows).
#
#   irm https://github.com/graphersal/graphersal-memory/releases/latest/download/install.ps1 | iex
#   & ([scriptblock]::Create((irm .../install.ps1))) -Version v0.2.1 -Dir C:\Tools
#
# It downloads the Windows archive, checks it against the release's SHA256SUMS, and puts
# graphersal-memory.exe into %LOCALAPPDATA%\Programs\graphersal-memory (or -Dir). It changes
# nothing else (not even PATH): run `graphersal-memory install` afterwards to connect it to your
# agent.
#
# Parameters (or environment variables):
#   -Version vX.Y.Z  the release to install (GRAPHERSAL_MEMORY_VERSION; default: the latest)
#   -Dir DIR         where to put the binary (GRAPHERSAL_MEMORY_INSTALL_DIR)
#   -BaseUrl URL     the releases URL (GRAPHERSAL_MEMORY_BASE_URL; default
#                    https://github.com/graphersal/graphersal-memory/releases); it holds
#                    latest/download/<file> and download/<tag>/<file>, as GitHub does

param(
    [string]$Version = $env:GRAPHERSAL_MEMORY_VERSION,
    [string]$Dir = $env:GRAPHERSAL_MEMORY_INSTALL_DIR,
    [string]$BaseUrl = $env:GRAPHERSAL_MEMORY_BASE_URL
)

# `irm | iex` runs this text in the caller's session: everything happens inside a function, so
# its preferences and variables stay out of that session.
function Install-GraphersalMemory([string]$Version, [string]$Dir, [string]$BaseUrl) {
    $ErrorActionPreference = 'Stop'
    Set-StrictMode -Version Latest

    function Fail([string]$Message) {
        Write-Host "graphersal-memory installer: $Message" -ForegroundColor Red
        throw "graphersal-memory installer: $Message"
    }

    $fromSource = 'cargo install --locked --git https://github.com/graphersal/graphersal-memory graphersal-memory'
    if (-not $BaseUrl) { $BaseUrl = 'https://github.com/graphersal/graphersal-memory/releases' }
    $BaseUrl = $BaseUrl.TrimEnd('/')
    if (-not $Dir) {
        if (-not $env:LOCALAPPDATA) { Fail 'LOCALAPPDATA is not set: give the target directory with -Dir' }
        $Dir = Join-Path $env:LOCALAPPDATA 'Programs\graphersal-memory'
    }
    if ($Version -eq 'latest') { $Version = '' }
    if ($Version -and -not $Version.StartsWith('v')) { $Version = "v$Version" }

    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($arch -ne [System.Runtime.InteropServices.Architecture]::X64) {
        # Windows on Arm runs the x86_64 build through its emulation.
        if ($arch -ne [System.Runtime.InteropServices.Architecture]::Arm64) {
            Fail "no prebuilt binary for Windows on $arch; build from source: $fromSource"
        }
    }
    $platform = 'windows-x86_64'

    # Older Windows PowerShell versions do not offer TLS 1.2 by default.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }

    function Fetch([string]$Url, [string]$OutFile) {
        $previous = $ProgressPreference
        $ProgressPreference = 'SilentlyContinue'
        try {
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
        } finally {
            $ProgressPreference = $previous
        }
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("graphersal-memory-" + [System.Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        if ($Version) {
            $releaseUrl = "$BaseUrl/download/$Version"
            $what = "release $Version"
        } else {
            $releaseUrl = "$BaseUrl/latest/download"
            $what = 'the latest release'
        }
        $sums = Join-Path $tmp 'SHA256SUMS'
        try {
            Fetch "$releaseUrl/SHA256SUMS" $sums
        } catch {
            Fail "cannot download SHA256SUMS of $what from $releaseUrl (no such release, or no network; you can build from source: $fromSource)"
        }
        $pattern = "^([0-9a-f]{64})  (graphersal-memory-(v[^ ]+)-$platform\.zip)$"
        $match = Get-Content $sums | ForEach-Object { [regex]::Match($_, $pattern) } |
            Where-Object { $_.Success } | Select-Object -First 1
        if (-not $match) { Fail "$what has no archive for $platform" }
        $expected = $match.Groups[1].Value
        $archive = $match.Groups[2].Value
        $foundVersion = $match.Groups[3].Value
        if ($Version -and $foundVersion -ne $Version) { Fail "SHA256SUMS of $Version names $archive" }
        # Download from the release that SHA256SUMS belongs to, even when "latest" moves meanwhile.
        if (-not $Version) { $releaseUrl = "$BaseUrl/download/$foundVersion" }

        Write-Host "Downloading graphersal-memory $foundVersion for $platform"
        $zip = Join-Path $tmp $archive
        try {
            Fetch "$releaseUrl/$archive" $zip
        } catch {
            Fail "cannot download $releaseUrl/$archive"
        }
        $actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            Fail "checksum mismatch for ${archive}: expected $expected, got $actual (the download is damaged or was changed; nothing was installed)"
        }
        Write-Host "Checksum verified (SHA-256 $expected)"

        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        $binary = Join-Path $tmp "graphersal-memory-$foundVersion-$platform\graphersal-memory.exe"
        if (-not (Test-Path $binary)) { Fail "$archive holds no graphersal-memory.exe" }

        New-Item -ItemType Directory -Force -Path $Dir | Out-Null
        $target = Join-Path $Dir 'graphersal-memory.exe'
        $upgrade = Test-Path $target
        # A running graphersal-memory.exe (its daemon) cannot be overwritten, but it can be renamed:
        # move it aside, put the new one in place, and remove the old one when it is free.
        if (Test-Path $target) {
            $old = Join-Path $Dir 'graphersal-memory.old.exe'
            Remove-Item -Force -ErrorAction SilentlyContinue $old
            Move-Item -Force $target $old
        }
        Copy-Item $binary $target
        Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $Dir 'graphersal-memory.old.exe')
        Write-Host "Installed $target"
        & $target --version
        if ($LASTEXITCODE -ne 0) { Fail 'the installed binary does not run on this system' }
    } finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $onPath = ($env:Path -split ';') + ($userPath -split ';') |
        Where-Object { $_ -and ($_.TrimEnd('\') -ieq $Dir.TrimEnd('\')) }
    $program = 'graphersal-memory'
    if (-not $onPath) {
        Write-Host ''
        Write-Host "$Dir is not on your PATH. To add it for your user (new terminals see it):"
        Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"$Dir;`" + [Environment]::GetEnvironmentVariable('Path', 'User'), 'User')"
        $program = "& '$target'"
    }
    Write-Host ''
    if ($upgrade) {
        Write-Host 'Upgraded. If it is set up in your agents, bring the daemon and the installed parts to this'
        Write-Host 'version with (`graphersal-memory upgrade` does both by itself):'
        Write-Host "  $program daemon restart"
        Write-Host "  $program install --refresh"
    } else {
        Write-Host 'Next step: connect it to your agents (Claude Code, opencode, Codex, Cursor, CodeWhale) with'
        Write-Host "  $program install"
    }
}

Install-GraphersalMemory -Version $Version -Dir $Dir -BaseUrl $BaseUrl
