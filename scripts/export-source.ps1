param([Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$destinationRoot=[IO.Path]::GetFullPath($Destination)
if($destinationRoot -eq $root -or $destinationRoot.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Source export destination must be outside the project'}
if(Test-Path -LiteralPath $destinationRoot){throw 'Source export destination already exists; existing content was preserved'}
$files=@()
foreach($name in @('Cargo.toml','Cargo.lock','build.rs','product.json','README.md','Start-GUI.cmd','Start-GUI.ps1','.gitignore','.cargo\config.toml')){
    $file=Join-Path $root $name
    if(-not(Test-Path -LiteralPath $file -PathType Leaf)){throw ('Required source file missing: '+$name)}
    $files+=Get-Item -LiteralPath $file
}
foreach($directory in @('src','tests','examples','installer','scripts','docs','.agents\skills')){
    $base=Join-Path $root $directory
    if(Test-Path -LiteralPath $base){
        $queue=[Collections.Generic.Queue[string]]::new();$queue.Enqueue($base)
        while($queue.Count -gt 0){
            foreach($file in Get-ChildItem -LiteralPath $queue.Dequeue() -Force){
                if(($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0){throw 'Source export does not follow reparse entries'}
                if($file.PSIsContainer){$queue.Enqueue($file.FullName)}
                elseif($file.Extension -in @('.rs','.toml','.nsi','.ps1','.md','.yaml','.yml','.json') -or ($directory -eq 'docs' -and $file.Extension -in @('.html','.svg','.png','.csv'))){$files+=$file}
            }
        }
    }
}
$files=@($files | Sort-Object FullName -Unique)
$entries=@()
foreach($file in $files){
    $relative=$file.FullName.Substring($root.Length+1)
    if($relative -match '(^|[\\/])(target|artifacts|data|\.git|\.codex|\.wix)([\\/]|$)' -or $file.Extension -in @('.exe','.dll','.pdb','.reaidx')){throw ('Forbidden source export file: '+$relative)}
    $entries+=@{source=$file.FullName;relative=$relative;hash=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash}
}
New-Item -ItemType Directory -Path $destinationRoot | Out-Null
$manifest=[Collections.Generic.List[string]]::new()
foreach($entry in $entries){
    $target=[IO.Path]::GetFullPath((Join-Path $destinationRoot $entry.relative))
    if(-not $target.StartsWith($destinationRoot+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Source export path escaped destination'}
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($target)) -Force | Out-Null
    if((Get-FileHash -LiteralPath $entry.source -Algorithm SHA256).Hash -ne $entry.hash){throw 'Source changed during export'}
    Copy-Item -LiteralPath $entry.source -Destination $target
    if((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $entry.hash){throw 'Source export copy mismatch'}
    $manifest.Add($entry.hash+'  '+$entry.relative.Replace('\','/'))
}
[IO.File]::WriteAllLines((Join-Path $destinationRoot 'SOURCE-MANIFEST.sha256'),$manifest,[Text.UTF8Encoding]::new($false))
[pscustomobject]@{destination=$destinationRoot;files=$entries.Count;manifest='SOURCE-MANIFEST.sha256';git_initialized=$false;user_data_included=$false} | ConvertTo-Json
