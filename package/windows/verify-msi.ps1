param(
    [Parameter(Mandatory = $true)]
    [string]$MsiPath,

    [string]$PreviousMsiPath,

    [string]$LogDirectory = "target/msi-verification"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$msi = (Resolve-Path $MsiPath).Path
$previousMsi = if ($PreviousMsiPath) {
    (Resolve-Path $PreviousMsiPath).Path
} else {
    $null
}
$logs = [System.IO.Path]::GetFullPath($LogDirectory)
New-Item -ItemType Directory -Force $logs | Out-Null

$installRoot = Join-Path $env:ProgramFiles "Clipboard Transformer"
$cliDirectory = Join-Path $installRoot "bin"
$expectedPath = $cliDirectory.TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar
)
$cli = Join-Path $cliDirectory "clipboard-transformer.exe"
$app = Join-Path $installRoot "Clipboard Transformer.exe"
$shortcut = Join-Path $env:ProgramData `
    "Microsoft\Windows\Start Menu\Programs\Clipboard Transformer\Clipboard Transformer.lnk"
$toastRegistration = `
    "Registry::HKEY_LOCAL_MACHINE\Software\Classes\CLSID\{B87B8C6D-2489-4A7D-9EFA-D02C54DD2390}\LocalServer32"

function Invoke-Msi {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("install", "uninstall")]
        [string]$Operation,

        [Parameter(Mandatory = $true)]
        [string]$PackagePath,

        [Parameter(Mandatory = $true)]
        [string]$LogPath
    )

    $switch = if ($Operation -eq "install") { "/i" } else { "/x" }
    $arguments = "$switch `"$PackagePath`" /qn /norestart /l*v `"$LogPath`""
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $isAdministrator = $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
    $startParameters = @{
        FilePath = "msiexec.exe"
        ArgumentList = $arguments
        Wait = $true
        PassThru = $true
    }
    if (-not $isAdministrator) {
        if ($env:GITHUB_ACTIONS -eq "true") {
            throw "MSI verification requires an elevated Windows runner; UAC prompts are unavailable in GitHub Actions"
        }
        $startParameters["Verb"] = "RunAs"
    }

    $process = Start-Process @startParameters

    if ($process.ExitCode -notin @(0, 3010)) {
        throw "MSI $Operation failed with exit code $($process.ExitCode); see $LogPath"
    }
}

function Get-MachinePathEntries {
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if ([string]::IsNullOrWhiteSpace($machinePath)) {
        return @()
    }

    return @(
        $machinePath -split ";" |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object {
                $_.Trim().TrimEnd([System.IO.Path]::DirectorySeparatorChar)
            }
    )
}

$installed = $false
$previousInstallLog = Join-Path $logs "previous-install.log"
$installLog = Join-Path $logs "install.log"
$uninstallLog = Join-Path $logs "uninstall.log"

try {
    if ($previousMsi) {
        Invoke-Msi `
            -Operation install `
            -PackagePath $previousMsi `
            -LogPath $previousInstallLog
        $installed = $true
    }

    Invoke-Msi `
        -Operation install `
        -PackagePath $msi `
        -LogPath $installLog
    $installed = $true

    if (-not (Test-Path $app)) {
        throw "MSI did not install the desktop executable at $app"
    }
    if (-not (Test-Path $cli)) {
        throw "MSI did not install the CLI executable at $cli"
    }
    if (-not (Test-Path $shortcut)) {
        throw "MSI did not create the Start Menu shortcut at $shortcut"
    }
    if (-not (Test-Path $toastRegistration)) {
        throw "MSI did not create the toast activation registration"
    }

    & $cli --version
    if ($LASTEXITCODE -ne 0) {
        throw "Installed CLI exited with code $LASTEXITCODE"
    }

    $pathEntry = Get-MachinePathEntries |
        Where-Object { $_ -ieq $expectedPath } |
        Select-Object -First 1
    if (-not $pathEntry) {
        throw "MSI did not add $cliDirectory to the machine PATH"
    }
}
finally {
    if ($installed) {
        Get-Process -Name "Clipboard Transformer", "clipboard-transformer" `
            -ErrorAction SilentlyContinue |
            Stop-Process -Force
        Invoke-Msi `
            -Operation uninstall `
            -PackagePath $msi `
            -LogPath $uninstallLog

        if (Test-Path $app) {
            throw "MSI uninstall left the desktop executable at $app"
        }
        if (Test-Path $cli) {
            throw "MSI uninstall left the CLI executable at $cli"
        }
        if (Test-Path $shortcut) {
            throw "MSI uninstall left the Start Menu shortcut at $shortcut"
        }
        if (Test-Path $toastRegistration) {
            throw "MSI uninstall left the toast activation registration"
        }

        $remainingPathEntry = Get-MachinePathEntries |
            Where-Object { $_ -ieq $expectedPath } |
            Select-Object -First 1
        if ($remainingPathEntry) {
            throw "MSI uninstall left $cliDirectory in the machine PATH"
        }
    }
}

$mode = if ($previousMsi) { "upgrade, " } else { "" }
Write-Output "MSI ${mode}install, CLI, PATH, Start Menu, toast registration, and uninstall verified."
