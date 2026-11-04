# Removes Claude Buddy and its Claude Code hooks. Other hooks stay as they are.
#   irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/uninstall.ps1 | iex
# Set $env:CLAUDE_BUDDY_PURGE = "1" first to also delete settings, usage data and added characters.

$ErrorActionPreference = "Stop"

$Dir = Join-Path $env:LOCALAPPDATA "Programs\ClaudeBuddy"
$Exe = Join-Path $Dir "claude-buddy-rs.exe"
$Shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Claude Buddy.lnk"

if (Test-Path -LiteralPath $Exe) {
    # First, while the path the hooks point at still exists: Claude Code must stop calling it.
    $Hooks = Start-Process $Exe -ArgumentList "--remove-hooks" -Wait -PassThru
    if ($Hooks.ExitCode -ne 0) { Write-Warning "Could not remove the hooks from Claude Code's settings.json." }
    Get-Process claude-buddy-rs -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe } | ForEach-Object {
        Stop-Process -Id $_.Id -Force
        $_.WaitForExit(5000) | Out-Null
    }
    Remove-Item -LiteralPath $Dir -Recurse -Force
}
Remove-Item -LiteralPath $Shortcut -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "ClaudeBuddyRS" -ErrorAction SilentlyContinue

if ($env:CLAUDE_BUDDY_PURGE -eq "1") {
    Remove-Item -LiteralPath (Join-Path $env:APPDATA "claude-buddy-rs") -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "Claude Buddy was removed."
