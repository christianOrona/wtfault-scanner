<#
  Save vehicle scorecard snapshots and diff two of them using the core's HTTP API.

  Usage:
    .\scorecard.ps1 -Vin <VIN>
    .\scorecard.ps1 -Diff <before.json>, <after.json>

  Parameters:
    -Vin: Save a snapshot of this VIN.
    -Diff: Exactly two snapshot file paths, the earlier one first, to compare.
    -Api: The core's API base URL. Default is 'http://127.0.0.1:8787/api/v1'.
    -OutDir: Where snapshots are written. Default is 'scorecards'.

  Examples:
    .\scorecard.ps1 -Vin "1HGBH41JXMN109186"
    .\scorecard.ps1 -Diff "scorecards/1HGBH41JXMN109186-20230401-120000.json", "scorecards/1HGBH41JXMN109186-20230401-130000.json"
#>
[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Vin,
    [string[]]$Diff,
    [string]$Api = 'http://127.0.0.1:8787/api/v1',
    [string]$OutDir = 'scorecards'
)

# An unset [string] parameter is '' rather than $null, so test what was actually passed.
$wantSnapshot = -not [string]::IsNullOrWhiteSpace($Vin)
$wantDiff = $PSBoundParameters.ContainsKey('Diff')
if ($wantSnapshot -eq $wantDiff) {
    Write-Host "Usage: scorecard.ps1 -Vin <VIN>   |   scorecard.ps1 -Diff <before.json>, <after.json>" -ForegroundColor Red
    exit 2
}

if ($wantSnapshot) {

    $ErrorActionPreference = 'Stop'
    try {
        $card = Invoke-RestMethod -Uri "$Api/vehicles/scorecard?vin=$([uri]::EscapeDataString($Vin.Trim()))" -Method Get
    }
    catch {
        Write-Host "could not reach the core at $Api - is it running? ($($_.Exception.Message))" -ForegroundColor Red
        exit 1
    }

    New-Item -ItemType Directory -Force -Path $OutDir > $null

    $path = Join-Path $OutDir ("{0}-{1}.json" -f $card.vin, (Get-Date -Format 'yyyyMMdd-HHmmss'))
    $card | ConvertTo-Json -Depth 10 | Set-Content -Path $path -Encoding UTF8

    Write-Host "Saved to $path"
    Write-Host ("identity: {0} settled, {1} contested, {2} unresolved | modules: {3} found, {4} named | findings: {5} established" -f @($card.identity.settled).Count, @($card.identity.contested).Count, @($card.identity.unresolved).Count, $card.modules.found, $card.modules.named, $card.findings.established)
}
else {
    # `powershell -File scorecard.ps1 -Diff a.json,b.json` passes one comma-joined string.
    if ($Diff.Count -eq 1) { $Diff = @($Diff[0] -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
    if ($Diff.Count -ne 2) {
        Write-Host "Usage: scorecard.ps1 -Diff <before.json>, <after.json>  (two paths, comma-separated)" -ForegroundColor Red
        exit 2
    }

    $ErrorActionPreference = 'Stop'
    if (-not (Test-Path $Diff[0])) {
        Write-Host "File not found: $($Diff[0])" -ForegroundColor Red
        exit 1
    }
    if (-not (Test-Path $Diff[1])) {
        Write-Host "File not found: $($Diff[1])" -ForegroundColor Red
        exit 1
    }

    $before = Get-Content -Raw $Diff[0] | ConvertFrom-Json
    $after = Get-Content -Raw $Diff[1] | ConvertFrom-Json

    try {
        $diffResult = Invoke-RestMethod -Uri "$Api/vehicles/scorecard/diff" -Method Post -ContentType 'application/json' -Body (@{ before = $before; after = $after } | ConvertTo-Json -Depth 10)
    }
    catch {
        Write-Host "could not reach the core at $Api - is it running? ($($_.Exception.Message))" -ForegroundColor Red
        exit 1
    }

    $gainedTotal = 0
    $lostTotal = 0

    foreach ($section in @('identity', 'modules', 'findings', 'as_built')) {
        Write-Host "${section}:"
        $sectionData = $diffResult.$section
        if ($null -ne $sectionData.gained) {
            foreach ($item in $sectionData.gained) {
                Write-Host "  + $item" -ForegroundColor Green
            }
            $gainedTotal += $sectionData.gained.Count
        }
        else {
            $sectionData.gained = @()
        }

        if ($null -ne $sectionData.lost) {
            foreach ($item in $sectionData.lost) {
                Write-Host "  - $item" -ForegroundColor Red
            }
            $lostTotal += $sectionData.lost.Count
        }
        else {
            $sectionData.lost = @()
        }

        if ($null -ne $sectionData.unchanged) {
            foreach ($item in $sectionData.unchanged) {
                Write-Host "    $item" -ForegroundColor DarkGray
            }
        }
        else {
            $sectionData.unchanged = @()
        }

        if ($sectionData.gained.Count -eq 0 -and $sectionData.lost.Count -eq 0 -and $sectionData.unchanged.Count -eq 0) {
            Write-Host "  (nothing)"
        }
    }

    Write-Host "gained $($gainedTotal), lost $($lostTotal)"

    if ($lostTotal -gt 0) {
        exit 1
    }
    else {
        exit 0
    }
}
