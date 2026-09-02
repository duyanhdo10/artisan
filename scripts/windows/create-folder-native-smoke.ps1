param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [Parameter(Mandatory = $true)]
    [string]$Identifier,

    [switch]$CleanStaleProfile
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms

$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$fixtureRoot = Join-Path $tempParent ("astian-create-folder-smoke-" + [Guid]::NewGuid().ToString("N"))
$vault = Join-Path $fixtureRoot "SmokeVault"
$localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$appData = Join-Path $localAppData $Identifier
$processes = [Collections.Generic.List[Diagnostics.Process]]::new()
$folderName = "K" + [char]0x1EBF + " ho" + [char]0x1EA1 + "ch"
$collisionName = "k" + [char]0x1EBF + " ho" + [char]0x1EA1 + "ch"

function Wait-Until {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Condition,
        [int]$TimeoutSeconds = 10,
        [string]$FailureMessage = "Timed out waiting for native UI state."
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $result = & $Condition
        if ($null -ne $result -and $result -ne $false) {
            return $result
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw $FailureMessage
}

function Get-IsolatedProcesses {
    return @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -in @("astian.exe", "msedgewebview2.exe") -and
        $null -ne $_.CommandLine -and
        $_.CommandLine.Contains($Identifier)
    })
}

function Start-SmokeApp {
    $process = Start-Process -FilePath $resolvedExecutable -PassThru -WindowStyle Hidden
    $processes.Add($process)
    $window = Wait-Until -TimeoutSeconds 15 -FailureMessage "Astian window did not become available." -Condition {
        $condition = [Windows.Automation.PropertyCondition]::new(
            [Windows.Automation.AutomationElement]::ProcessIdProperty,
            $process.Id
        )
        [Windows.Automation.AutomationElement]::RootElement.FindFirst(
            [Windows.Automation.TreeScope]::Children,
            $condition
        )
    }
    return @{ Process = $process; Window = $window }
}

function Find-NamedElement {
    param(
        [Parameter(Mandatory = $true)]
        [Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Windows.Automation.ControlType]$ControlType = $null,
        [int]$TimeoutSeconds = 10
    )

    return Wait-Until -TimeoutSeconds $TimeoutSeconds -FailureMessage "UI element '$Name' was not found." -Condition {
        $nameCondition = [Windows.Automation.PropertyCondition]::new(
            [Windows.Automation.AutomationElement]::NameProperty,
            $Name
        )
        $condition = $nameCondition
        if ($null -ne $ControlType) {
            $typeCondition = [Windows.Automation.PropertyCondition]::new(
                [Windows.Automation.AutomationElement]::ControlTypeProperty,
                $ControlType
            )
            $condition = [Windows.Automation.AndCondition]::new($nameCondition, $typeCondition)
        }
        $Root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
    }
}

function Invoke-Element {
    param([Windows.Automation.AutomationElement]$Element)
    $pattern = $null
    if ($Element.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        ([Windows.Automation.InvokePattern]$pattern).Invoke()
        return
    }
    $Element.SetFocus()
    [Windows.Forms.SendKeys]::SendWait("{ENTER}")
}

function Set-ElementValue {
    param(
        [Windows.Automation.AutomationElement]$Element,
        [string]$Value
    )
    $pattern = $null
    if ($Element.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) {
        ([Windows.Automation.ValuePattern]$pattern).SetValue($Value)
        return
    }
    $Element.SetFocus()
    [Windows.Forms.SendKeys]::SendWait("^a")
    [Windows.Forms.SendKeys]::SendWait($Value)
}

function Close-SmokeApp {
    param([Diagnostics.Process]$Process)
    if ($Process.HasExited) { return }
    [void]$Process.CloseMainWindow()
    if (-not $Process.WaitForExit(10000)) {
        throw "Astian did not close through its native window path."
    }
}

$result = [ordered]@{
    restoredVault = $false
    unicodeCreate = $false
    collisionPreserved = $false
    externalFolderObserved = $false
    restartVisible = $false
    vaultArtifacts = -1
}

try {
    if (Test-Path -LiteralPath $appData) {
        if (-not $CleanStaleProfile) {
            throw "Isolated app-data directory already exists: $appData"
        }
        foreach ($staleProcess in Get-IsolatedProcesses) {
            Stop-Process -Id $staleProcess.ProcessId -Force -ErrorAction SilentlyContinue
        }
        Start-Sleep -Milliseconds 500
        Remove-Item -LiteralPath $appData -Recurse -Force
    }
    [void](New-Item -ItemType Directory -Path $vault -Force)
    [void](New-Item -ItemType Directory -Path $appData -Force)
    $settings = [ordered]@{
        schema_version = 1
        recent_vaults = @($vault)
    } | ConvertTo-Json -Depth 3
    [IO.File]::WriteAllText(
        (Join-Path $appData "settings.json"),
        $settings + "`n",
        [Text.UTF8Encoding]::new($false)
    )

    $first = Start-SmokeApp
    [void](Find-NamedElement -Root $first.Window -Name "Create folder" -ControlType ([Windows.Automation.ControlType]::Button) -TimeoutSeconds 15)
    $result.restoredVault = $true

    Invoke-Element (Find-NamedElement -Root $first.Window -Name "Create folder" -ControlType ([Windows.Automation.ControlType]::Button))
    $folderInput = Find-NamedElement -Root $first.Window -Name "New root folder name" -ControlType ([Windows.Automation.ControlType]::Edit)
    Set-ElementValue -Element $folderInput -Value $folderName
    Invoke-Element (Find-NamedElement -Root $first.Window -Name "Create" -ControlType ([Windows.Automation.ControlType]::Button))
    [void](Wait-Until -FailureMessage "Unicode folder was not created." -Condition {
        Test-Path -LiteralPath (Join-Path $vault $folderName) -PathType Container
    })
    [void](Find-NamedElement -Root $first.Window -Name $folderName)
    $result.unicodeCreate = $true

    Invoke-Element (Find-NamedElement -Root $first.Window -Name "Create folder" -ControlType ([Windows.Automation.ControlType]::Button))
    $collisionInput = Find-NamedElement -Root $first.Window -Name "New root folder name" -ControlType ([Windows.Automation.ControlType]::Edit)
    Set-ElementValue -Element $collisionInput -Value $collisionName
    Invoke-Element (Find-NamedElement -Root $first.Window -Name "Create" -ControlType ([Windows.Automation.ControlType]::Button))
    [void](Find-NamedElement -Root $first.Window -Name "A file or folder already uses that name. Choose another name.")
    $matchingFolders = @(Get-ChildItem -LiteralPath $vault -Directory | Where-Object {
        $_.Name -ieq $folderName
    })
    if ($matchingFolders.Count -ne 1) {
        throw "Case-collision request changed the destination namespace."
    }
    $result.collisionPreserved = $true

    [void](New-Item -ItemType Directory -Path (Join-Path $vault "External\Empty") -Force)
    [void](Find-NamedElement -Root $first.Window -Name "External" -TimeoutSeconds 15)
    [void](Find-NamedElement -Root $first.Window -Name "Empty" -TimeoutSeconds 15)
    $result.externalFolderObserved = $true

    Close-SmokeApp -Process $first.Process
    $second = Start-SmokeApp
    [void](Find-NamedElement -Root $second.Window -Name $folderName -TimeoutSeconds 15)
    [void](Find-NamedElement -Root $second.Window -Name "External")
    [void](Find-NamedElement -Root $second.Window -Name "Empty")
    $result.restartVisible = $true
    Close-SmokeApp -Process $second.Process

    $artifacts = @(Get-ChildItem -LiteralPath $vault -Recurse -Force | Where-Object {
        $_.Name -like ".astian-*" -or $_.Name -in @("settings.json", "session.json")
    })
    $result.vaultArtifacts = $artifacts.Count
    if ($result.vaultArtifacts -ne 0) {
        throw "Astian internal artifacts appeared in the vault."
    }

    $result | ConvertTo-Json -Compress
}
finally {
    foreach ($process in $processes) {
        if (-not $process.HasExited) {
            & taskkill.exe /PID $process.Id /T /F *> $null
        }
    }
    foreach ($isolatedProcess in Get-IsolatedProcesses) {
        Stop-Process -Id $isolatedProcess.ProcessId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500

    $resolvedFixture = [IO.Path]::GetFullPath($fixtureRoot)
    if ($resolvedFixture.StartsWith($tempParent, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedFixture).StartsWith("astian-create-folder-smoke-")) {
        Remove-Item -LiteralPath $resolvedFixture -Recurse -Force -ErrorAction SilentlyContinue
    }

    $resolvedAppData = [IO.Path]::GetFullPath($appData)
    if ((Split-Path -Parent $resolvedAppData) -eq [IO.Path]::GetFullPath($localAppData) -and
        (Split-Path -Leaf $resolvedAppData) -eq $Identifier) {
        Remove-Item -LiteralPath $resolvedAppData -Recurse -Force -ErrorAction SilentlyContinue
    }
}
