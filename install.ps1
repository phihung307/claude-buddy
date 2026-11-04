# Installs Claude Buddy for the current user. No admin rights needed.
#   irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/install.ps1 | iex
# Set $env:CLAUDE_BUDDY_NO_HOOKS = "1" first to skip connecting to Claude Code.
# -FromFile installs a local build instead of downloading, for testing.
param([string]$FromFile)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "taivt0109/claude-buddy"
$Asset = "claude-buddy-rs.exe"
$Dir = Join-Path $env:LOCALAPPDATA "Programs\ClaudeBuddy"
$Exe = Join-Path $Dir $Asset
$Shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Claude Buddy.lnk"

$Temp = Join-Path $env:TEMP "claude-buddy-install-$PID.exe"
try {
    if ($FromFile) {
        Copy-Item -LiteralPath $FromFile $Temp -Force
    } else {
        $Base = "https://github.com/$Repo/releases/latest/download"
        Write-Host "Downloading Claude Buddy..."
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest "$Base/$Asset" -OutFile $Temp -UseBasicParsing
        $Expected = ((Invoke-WebRequest "$Base/$Asset.sha256" -UseBasicParsing).Content -split "\s+")[0]
        $Actual = (Get-FileHash $Temp -Algorithm SHA256).Hash
        if ($Expected -ine $Actual) { throw "Checksum mismatch: expected $Expected, got $Actual. Nothing was installed." }
    }

    # An older copy still running would lock the file.
    Get-Process claude-buddy-rs -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe } | ForEach-Object {
        Stop-Process -Id $_.Id -Force
        $_.WaitForExit(5000) | Out-Null
    }
    New-Item -ItemType Directory -Force $Dir | Out-Null
    Move-Item -LiteralPath $Temp $Exe -Force
} finally {
    Remove-Item -LiteralPath $Temp -Force -ErrorAction SilentlyContinue
}

$Shell = New-Object -ComObject WScript.Shell
$Link = $Shell.CreateShortcut($Shortcut)
$Link.TargetPath = $Exe
$Link.WorkingDirectory = $Dir
$Link.Save()

if ($env:CLAUDE_BUDDY_NO_HOOKS -eq "1") {
    Write-Host "Skipped connecting to Claude Code. The app will ask when it starts."
} else {
    $Hooks = Start-Process $Exe -ArgumentList "--install-hooks" -Wait -PassThru
    if ($Hooks.ExitCode -eq 0) {
        Write-Host "Connected to Claude Code (settings.json was backed up first). Sessions opened from now on will show up."
    } else {
        Write-Warning "Could not update Claude Code's settings.json. Connect from the app's Settings page instead."
    }
}

Start-Process $Exe
Write-Host "Claude Buddy is installed in $Dir"
Write-Host "To remove it: irm https://raw.githubusercontent.com/$Repo/main/uninstall.ps1 | iex"
