param(
    [string]$NsisPath,
    [string]$OutputDirectory,
    [ValidateRange(1,32)][int]$Jobs=2
)
$ErrorActionPreference='Stop'
$projectRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$product=Get-Content -LiteralPath (Join-Path $projectRoot 'product.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if(-not $OutputDirectory){$OutputDirectory=Join-Path $projectRoot 'artifacts\release'}
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
if(-not $NsisPath){$command=Get-Command makensis -ErrorAction SilentlyContinue;if($command){$NsisPath=$command.Source}}
if(-not $NsisPath -or -not(Test-Path -LiteralPath $NsisPath)){throw 'NSIS 3.13 is required. Supply -NsisPath with the path to makensis.exe.'}
$NsisPath=[IO.Path]::GetFullPath($NsisPath)
$nsisVersion=(& $NsisPath /VERSION | Out-String).Trim()
if($LASTEXITCODE -ne 0 -or $nsisVersion -ne 'v3.13'){throw ('Expected NSIS v3.13, got '+$nsisVersion)}
. (Join-Path $PSScriptRoot 'pe-info.ps1')
Push-Location $projectRoot
try {
    $metadataText=& cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if($LASTEXITCODE -ne 0){throw 'cargo metadata failed'}
    $metadata=$metadataText | ConvertFrom-Json
    $package=$metadata.packages | Where-Object { [IO.Path]::GetFullPath($_.manifest_path) -eq (Join-Path $projectRoot 'Cargo.toml') }
    if(@($package).Count -ne 1){throw 'Ambiguous Finder package'}
    $messages=& cargo build --release --locked --bins -j $Jobs --message-format=json
    if($LASTEXITCODE -ne 0){throw ($messages -join [Environment]::NewLine)}
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $buildMessages=@($messages | ForEach-Object {$_ | ConvertFrom-Json})
    $build=$buildMessages | Where-Object {$_.reason -eq 'build-script-executed' -and $_.package_id -eq $package.id} | Select-Object -Last 1
    if(-not $build){throw 'Finder build-script output is missing'}
    $workRoot=Join-Path $projectRoot 'artifacts\package-work'
    New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
    $payload=Join-Path $workRoot ([guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $payload | Out-Null
    $binaries=@()
    foreach($item in @(@{name='finder';subsystem=2},@{name='finder-cli';subsystem=3})) {
        $artifact=$buildMessages | Where-Object {$_.reason -eq 'compiler-artifact' -and $_.package_id -eq $package.id -and $_.target.name -eq $item.name} | Select-Object -Last 1
        $executable=$artifact.executable
        if(-not $executable){throw ('Missing binary '+$item.name)}
        $info=Get-FinderPeInfo $executable
        if($info.subsystem -ne $item.subsystem -or $info.version.ProductName -ne $product.name -or $info.version.CompanyName -ne $product.publisher){throw 'Executable identity or subsystem mismatch'}
        if($info.imports | Where-Object {$_ -match '^(vcruntime|msvcp|ucrtbase|api-ms-win-crt)'} ){throw 'Finder still imports the VC++ runtime dynamically'}
        $destination=Join-Path $payload ($item.name+'.exe')
        Copy-Item -LiteralPath $executable -Destination $destination
        $versionOut=Join-Path $payload ($item.name+'-version.txt')
        $versionErr=Join-Path $payload ($item.name+'-version.stderr.txt')
        $process=Start-Process -FilePath $destination -ArgumentList '--version' -WindowStyle Hidden -PassThru -RedirectStandardOutput $versionOut -RedirectStandardError $versionErr
        $processHandle=$process.Handle
        $process.WaitForExit()
        $version=(Get-Content -LiteralPath $versionOut -Raw).Trim()
        if($process.ExitCode -ne 0 -or $version -ne ('finder '+$package.version)){throw 'Finder version check failed'}
        $binaries+=@{name=$item.name+'.exe';bytes=$info.bytes;sha256=$info.sha256;architecture=$info.architecture;subsystem=$info.subsystem;company=$info.version.CompanyName;product=$info.version.ProductName;direct_imports=$info.imports}
    }
    Copy-Item -LiteralPath (Join-Path $build.out_dir 'finder.ico') -Destination (Join-Path $payload 'finder.ico')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination (Join-Path $payload 'README.md')
    [IO.File]::WriteAllText((Join-Path $payload $product.install_marker),$product.id,[Text.UTF8Encoding]::new($false))
    $packages=@{};foreach($dependency in $metadata.packages){$packages[$dependency.id]=$dependency}
    $nodes=@{};foreach($node in $metadata.resolve.nodes){$nodes[$node.id]=$node}
    $seen=[Collections.Generic.HashSet[string]]::new()
    $pending=[Collections.Generic.Queue[string]]::new();$pending.Enqueue($package.id)
    while($pending.Count -gt 0){$id=$pending.Dequeue();if(-not $seen.Add($id)){continue};foreach($dependency in $nodes[$id].deps){if(@($dependency.dep_kinds | Where-Object {$null -eq $_.kind}).Count -gt 0){$pending.Enqueue($dependency.pkg)}}}
    $notices=[Text.StringBuilder]::new()
    [void]$notices.AppendLine('Finder third-party notices')
    [void]$notices.AppendLine('Generated from the locked Windows normal dependency closure. The license texts below remain owned by their respective authors.')
    foreach($dependency in @($seen | ForEach-Object {$packages[$_]} | Where-Object {$_.id -ne $package.id} | Sort-Object name,version)){
        [void]$notices.AppendLine(([Environment]::NewLine+'===== '+$dependency.name+' '+$dependency.version+' ====='))
        [void]$notices.AppendLine(('License: '+$dependency.license))
        [void]$notices.AppendLine(('Repository: '+$dependency.repository))
        [void]$notices.AppendLine(('Authors: '+($dependency.authors -join ', ')))
        $dependencyRoot=[IO.Path]::GetDirectoryName($dependency.manifest_path)
        $licenseFiles=@(Get-ChildItem -LiteralPath $dependencyRoot -File | Where-Object {$_.Name -match '^(LICENSE|COPYING|NOTICE|UNLICENSE)([._-]|$)'})
        if($dependency.license_file){$explicit=Join-Path $dependencyRoot $dependency.license_file;if(Test-Path -LiteralPath $explicit){$licenseFiles+=Get-Item -LiteralPath $explicit}}
        foreach($licenseFile in @($licenseFiles | Sort-Object FullName -Unique)){
            if($licenseFile.Length -gt 1048576){throw ('License text exceeds expected size: '+$licenseFile.Name)}
            [void]$notices.AppendLine(('--- '+$licenseFile.Name+' ---'))
            [void]$notices.AppendLine([IO.File]::ReadAllText($licenseFile.FullName))
        }
    }
    $nsisRoot=[IO.Path]::GetDirectoryName($NsisPath)
    if(-not(Test-Path -LiteralPath (Join-Path $nsisRoot 'COPYING'))){$nsisRoot=[IO.Path]::GetDirectoryName($nsisRoot)}
    [void]$notices.AppendLine('===== NSIS installer license =====')
    [void]$notices.AppendLine([IO.File]::ReadAllText((Join-Path $nsisRoot 'COPYING')))
    [IO.File]::WriteAllText((Join-Path $payload 'THIRD-PARTY-NOTICES.txt'),$notices.ToString(),[Text.UTF8Encoding]::new($false))
    $installer=Join-Path $OutputDirectory ('Finder-'+$package.version+'-Setup-x64.exe')
    if(Test-Path -LiteralPath $installer){throw 'Installer output already exists; move it aside before creating another release'}
    $arguments=@('/NOCONFIG','/WX','/V3',('/DPAYLOAD_DIR='+$payload),('/DOUTPUT_FILE='+$installer),('/DPRODUCT_NAME='+$product.name),('/DPUBLISHER='+$product.publisher),('/DPRODUCT_ID='+$product.id),('/DINSTALL_MARKER='+$product.install_marker),('/DPRODUCT_VERSION='+$package.version),(Join-Path $projectRoot 'installer\finder.nsi'))
    & $NsisPath @arguments
    if($LASTEXITCODE -ne 0){throw 'NSIS installer build failed'}
    $portable=Join-Path $OutputDirectory ('Finder-'+$package.version+'-Portable-x64.zip')
    if(Test-Path -LiteralPath $portable){throw 'Portable output already exists; move it aside before creating another release'}
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip=[IO.Compression.ZipFile]::Open($portable,[IO.Compression.ZipArchiveMode]::Create)
    $portableFiles=@('finder.exe','finder-cli.exe','README.md','THIRD-PARTY-NOTICES.txt')
    try{foreach($name in $portableFiles){[void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,(Join-Path $payload $name),$name,[IO.Compression.CompressionLevel]::Optimal)}}finally{$zip.Dispose()}
    $payloadHashes=@{};foreach($name in @($portableFiles)+@('finder.ico',$product.install_marker)){$payloadHashes[$name]=(Get-FileHash -LiteralPath (Join-Path $payload $name) -Algorithm SHA256).Hash}
    $result=[ordered]@{product=$product.name;publisher=$product.publisher;version=$package.version;nsis_version=$nsisVersion;installer=$installer;installer_sha256=(Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash;installer_bytes=(Get-Item -LiteralPath $installer).Length;portable=$portable;portable_sha256=(Get-FileHash -LiteralPath $portable -Algorithm SHA256).Hash;portable_bytes=(Get-Item -LiteralPath $portable).Length;binaries=$binaries;payload=$payload;payload_hashes=$payloadHashes;signed=$false}
    $manifest=Join-Path $OutputDirectory ('Finder-'+$package.version+'-manifest.json')
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifest -Encoding UTF8
    $result | ConvertTo-Json -Depth 6
} finally {Pop-Location}
