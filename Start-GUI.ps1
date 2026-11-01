param([string]$Folder)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'finder.exe'
if (-not (Test-Path -LiteralPath $exe)) { $exe = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-msvc\release\finder.exe' }
if (-not (Test-Path -LiteralPath $exe)) { throw 'Release executable missing. Run cargo build --release --locked first.' }
$appArguments=@('gui')
if($PSBoundParameters.ContainsKey('Folder')) {
 if([string]::IsNullOrWhiteSpace($Folder) -or $Folder.Contains('"')) {throw 'Folder path is empty or contains quotation marks.'}
 $appArguments+=@('--folder',('"'+$Folder+'"'))
}
Start-Process -FilePath $exe -ArgumentList $appArguments -WorkingDirectory $PSScriptRoot -WindowStyle Hidden
