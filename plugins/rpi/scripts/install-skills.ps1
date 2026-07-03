param(
    [ValidateSet("codex", "claude", "both")]
    [string] $Target = "both",

    [string] $CodexSkillsDir,
    [string] $ClaudeSkillsDir
)

$ErrorActionPreference = "Stop"

$PluginRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$SourceDir = Join-Path $PluginRoot "skills"

if (-not $CodexSkillsDir) {
    if ($env:CODEX_HOME) {
        $CodexSkillsDir = Join-Path $env:CODEX_HOME "skills"
    } else {
        $CodexSkillsDir = Join-Path $HOME ".codex\skills"
    }
}

if (-not $ClaudeSkillsDir) {
    $ClaudeSkillsDir = Join-Path $HOME ".claude\skills"
}

$Destinations = @()
if ($Target -eq "codex" -or $Target -eq "both") {
    $Destinations += $CodexSkillsDir
}
if ($Target -eq "claude" -or $Target -eq "both") {
    $Destinations += $ClaudeSkillsDir
}

foreach ($Destination in $Destinations) {
    New-Item -ItemType Directory -Force $Destination | Out-Null
    foreach ($Skill in Get-ChildItem -Path $SourceDir -Directory) {
        $TargetPath = Join-Path $Destination $Skill.Name
        if (Test-Path $TargetPath) {
            Remove-Item -LiteralPath $TargetPath -Recurse -Force
        }
        Copy-Item -LiteralPath $Skill.FullName -Destination $TargetPath -Recurse
        Write-Host "Installed $($Skill.Name) -> $TargetPath"
    }
}
