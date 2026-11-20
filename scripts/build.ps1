param([switch]$Debug)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
Push-Location $project
try {
    # Local portable tools are optional. Normal Rust + Visual Studio installations also work.
    $portable = 'E:\ArcDeskBuild'
    $toolchain = @()
    if (Test-Path "$portable\mingw64\bin\gcc.exe") {
        $env:PATH = "$portable\mingw64\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
        $env:RUSTUP_HOME = "$portable\rustup"
        $env:CARGO_HOME = "$portable\cargo"
        $env:CARGO_TARGET_DIR = "$portable\target"
        $env:TEMP = "$portable\temp"; $env:TMP = $env:TEMP
        New-Item -ItemType Directory -Force $env:TEMP | Out-Null
        $toolchain = @('+stable-x86_64-pc-windows-gnu')
    }
    $buildArgs = @('build','--workspace','--locked')
    $profile = 'debug'
    if (!$Debug) { $buildArgs += '--release'; $profile = 'release' }
    & cargo @toolchain @buildArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $project 'target' }
    $dist = Join-Path $project 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    Copy-Item -LiteralPath "$target\$profile\xandesk.exe" -Destination (Join-Path $dist 'XanDesk.exe')
    Copy-Item -LiteralPath "$target\$profile\arcdesk-relay.exe" -Destination $dist
    if ($toolchain.Count) {
        foreach ($dll in @('libgcc_s_seh-1.dll','libstdc++-6.dll','libwinpthread-1.dll')) {
            Copy-Item -LiteralPath "$portable\mingw64\bin\$dll" -Destination $dist
        }
    }
    Copy-Item -LiteralPath "$project\README.md","$project\LICENSE","$project\QUICKSTART.txt","$project\SETUP-PAGES.md","$project\VALIDATION.md" -Destination $dist
    $metadataText = & cargo @toolchain metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-gnu
    if ($LASTEXITCODE -ne 0) { throw 'Could not generate dependency notices' }
    $metadata = $metadataText | ConvertFrom-Json
    $notices = @('# Third-party dependencies', '', 'This distribution includes software from these Rust packages. License texts are in licenses/.', '')
    foreach ($package in $metadata.packages) {
        if (!$package.source) { continue }
        $notices += "- $($package.name) $($package.version): $($package.license) — $($package.repository)"
        $sourceDir = Split-Path -Parent $package.manifest_path
        $licenseDir = Join-Path $dist "licenses\$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Force $licenseDir | Out-Null
        Get-ChildItem -LiteralPath $sourceDir -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' } | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $licenseDir
        }
        if ($package.name -eq 'openh264-sys2') {
            Get-ChildItem -LiteralPath (Join-Path $sourceDir 'upstream') -File | Where-Object { $_.Name -match '^(LICENSE|COPYING|NOTICE)' } | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $licenseDir }
        }
    }
    if ($toolchain.Count) {
        $runtimeLicenses = Join-Path $dist 'licenses\gnu-runtime'
        New-Item -ItemType Directory -Force $runtimeLicenses | Out-Null
        Get-ChildItem -LiteralPath (Join-Path $project 'licenses\gnu-runtime') -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $runtimeLicenses }
        Get-ChildItem -LiteralPath "$portable\mingw64\share" -Recurse -File | Where-Object { $_.Name -match '^COPYING(\.RUNTIME|\.LIB|3|\.mingw-w64)?$' } | ForEach-Object {
            $label = ($_.FullName.Substring($portable.Length) -replace '[:\\/]','_')
            Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $runtimeLicenses $label)
        }
        $notices += '- GNU compiler runtime DLLs: see licenses/gnu-runtime (GCC Runtime Library Exception and associated notices).'
    }
    $notices | Set-Content -LiteralPath (Join-Path $dist 'THIRD-PARTY-NOTICES.md')
    Write-Host "Portable build: $dist"
} finally { Pop-Location }

