param([Parameter(Mandatory=$true)][string]$Manifest)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$product=Get-Content -LiteralPath (Join-Path $root 'product.json') -Raw | ConvertFrom-Json
$release=Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json
if($release.product -ne $product.name -or $release.publisher -ne $product.publisher){throw 'Installer manifest identity mismatch'}
if((Get-FileHash -LiteralPath $release.installer -Algorithm SHA256).Hash -ne $release.installer_sha256){throw 'Installer hash mismatch'}
$keyPath='Software\Microsoft\Windows\CurrentVersion\Uninstall\'+$product.id
$registry=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser,[Microsoft.Win32.RegistryView]::Registry64)
$existing=$registry.OpenSubKey($keyPath)
if($existing){$existing.Dispose();$registry.Dispose();throw 'Finder is already registered for this user; integration test was not started'}
$programs=[Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$shortcut=Join-Path $programs ($product.publisher+'\'+$product.name+'.lnk')
$desktopShortcut=Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)) ($product.name+'.lnk')
if((Test-Path -LiteralPath $shortcut) -or (Test-Path -LiteralPath $desktopShortcut)){$registry.Dispose();throw 'A product shortcut already exists; integration test was not started'}
$testParent=Join-Path $root 'artifacts\installer-test'
New-Item -ItemType Directory -Path $testParent -Force | Out-Null
$directory=Join-Path $testParent ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $directory | Out-Null
function Invoke-HiddenAndWait([string]$File,[string[]]$Arguments){
    $process=Start-Process -FilePath $File -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    $handle=$process.Handle
    if(-not $process.WaitForExit(30000)){throw ('Owned test process did not finish; preserved for inspection: '+$process.Id)}
    if($process.ExitCode -ne 0){throw ('Installer test process failed with exit code '+$process.ExitCode)}
}
$watch=[Diagnostics.Stopwatch]::StartNew()
Invoke-HiddenAndWait $release.installer @('/S',('/D='+$directory))
foreach($name in @('finder.exe','finder-cli.exe','README.md','THIRD-PARTY-NOTICES.txt',$product.install_marker)){
    $file=Join-Path $directory $name
    if((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $release.payload_hashes.$name){throw ('Installed payload mismatch: '+$name)}
}
$registration=$registry.OpenSubKey($keyPath)
if(-not $registration){throw 'Installer did not create its user registration'}
try{
    if($registration.GetValue('Publisher') -ne $product.publisher -or $registration.GetValue('DisplayName') -ne $product.name -or $registration.GetValue('ProductId') -ne $product.id -or $registration.GetValue('InstallLocation') -ne $directory){throw 'Installed product registration mismatch'}
    if($registration.GetValue('UninstallString') -ne ('"'+$directory+'\Uninstall.exe"')){throw 'Registered uninstall command quoting mismatch'}
}finally{$registration.Dispose()}
$shell=New-Object -ComObject WScript.Shell
$link=$shell.CreateShortcut($shortcut)
if([IO.Path]::GetFullPath($link.TargetPath) -ne (Join-Path $directory 'finder.exe')){throw 'Start Menu shortcut target mismatch'}
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($link)
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
if(Test-Path -LiteralPath $desktopShortcut){throw 'Optional desktop shortcut was created by default'}
$data=Join-Path $directory 'data'
New-Item -ItemType Directory -Path $data | Out-Null
$sentinel=Join-Path $data 'preserved-user-data.txt'
[IO.File]::WriteAllText($sentinel,'Installer test: this owned data must survive uninstall')
$originalDirectory=$directory
$upgradeDirectory=Join-Path $testParent ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $upgradeDirectory | Out-Null
Invoke-HiddenAndWait $release.installer @('/S',('/D='+$upgradeDirectory))
$upgraded=$registry.OpenSubKey($keyPath)
try{if(-not $upgraded -or $upgraded.GetValue('InstallLocation') -ne $upgradeDirectory){throw 'Upgrade ignored the selected install directory'}}finally{if($upgraded){$upgraded.Dispose()}}
foreach($name in @('finder.exe','finder-cli.exe')){if((Get-FileHash -LiteralPath (Join-Path $upgradeDirectory $name) -Algorithm SHA256).Hash -ne $release.payload_hashes.$name){throw 'Upgrade payload mismatch'}}
Invoke-HiddenAndWait (Join-Path $originalDirectory 'Uninstall.exe') @('/S',('_?='+$originalDirectory))
$upgraded=$registry.OpenSubKey($keyPath)
try{if(-not $upgraded -or $upgraded.GetValue('InstallLocation') -ne $upgradeDirectory){throw 'Retired installation removed the new registration'}}finally{if($upgraded){$upgraded.Dispose()}}
$shell=New-Object -ComObject WScript.Shell
$link=$shell.CreateShortcut($shortcut)
if([IO.Path]::GetFullPath($link.TargetPath) -ne (Join-Path $upgradeDirectory 'finder.exe')){throw 'Retired installation removed or changed the current shortcut'}
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($link)
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
if([IO.File]::ReadAllText($sentinel) -ne 'Installer test: this owned data must survive uninstall'){throw 'Retired installation removed user data'}
$directory=$upgradeDirectory
$data=Join-Path $directory 'data'
New-Item -ItemType Directory -Path $data | Out-Null
$sentinel=Join-Path $data 'preserved-user-data.txt'
[IO.File]::WriteAllText($sentinel,'Installer test: this owned data must survive uninstall')
Invoke-HiddenAndWait (Join-Path $directory 'Uninstall.exe') @('/S',('_?='+$directory))
if((Test-Path -LiteralPath (Join-Path $directory 'finder.exe')) -or (Test-Path -LiteralPath (Join-Path $directory 'finder-cli.exe'))){throw 'Uninstaller left application files'}
$remaining=$registry.OpenSubKey($keyPath)
if($remaining){$remaining.Dispose();throw 'Uninstaller left its registration'}
$registry.Dispose()
if(Test-Path -LiteralPath $shortcut){throw 'Uninstaller left its Start Menu shortcut'}
if([IO.File]::ReadAllText($sentinel) -ne 'Installer test: this owned data must survive uninstall'){throw 'Uninstaller changed user data'}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip=[IO.Compression.ZipFile]::OpenRead($release.portable)
try{
    $expected=@('finder.exe','finder-cli.exe','README.md','THIRD-PARTY-NOTICES.txt')
    if($zip.Entries.Count -ne $expected.Count){throw 'Portable archive entry count mismatch'}
    foreach($entry in $zip.Entries){
        if($entry.FullName -notin $expected){throw 'Unexpected portable archive entry'}
        $stream=$entry.Open();$sha=[Security.Cryptography.SHA256]::Create()
        try{$hash=[BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-','')}finally{$stream.Dispose();$sha.Dispose()}
        if($hash -ne $release.payload_hashes.($entry.FullName)){throw 'Portable archive payload hash mismatch'}
    }
}finally{$zip.Dispose()}
$result=[ordered]@{installer=$release.installer;installer_sha256=$release.installer_sha256;test_directory=$directory;silent_install_passed=$true;payload_hashes_passed=$true;publisher=$product.publisher;start_menu_passed=$true;upgrade_directory_choice_passed=$true;retired_uninstaller_preserved_current_registration=$true;optional_desktop_not_created=$true;registered_uninstall_command_passed=$true;silent_uninstall_passed=$true;user_data_preserved=$true;registration_removed=$true;portable_hashes_passed=$true;elapsed_ms=$watch.Elapsed.TotalMilliseconds;scope='temporary per-user integration; GUI not launched; existing registrations and shortcuts rejected before testing'}
$result | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $directory 'verification.json') -Encoding UTF8
$result | ConvertTo-Json -Depth 3
