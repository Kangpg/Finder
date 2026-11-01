function Get-FinderPeInfo {
    param([Parameter(Mandatory=$true)][string]$Path)
    $bytes=[IO.File]::ReadAllBytes($Path)
    if($bytes.Length -lt 64){throw 'PE image is too short'}
    $pe=[BitConverter]::ToInt32($bytes,0x3c)
    if($pe -lt 0 -or $pe+94 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes,$pe) -ne 0x4550){throw 'Invalid PE image'}
    $machine=[BitConverter]::ToUInt16($bytes,$pe+4)
    $optional=$pe+24
    if($machine -ne 0x8664 -or [BitConverter]::ToUInt16($bytes,$optional) -ne 0x20b){throw 'Finder must be a Windows x64 PE image'}
    $sectionCount=[BitConverter]::ToUInt16($bytes,$pe+6)
    $sectionOffset=$optional+[BitConverter]::ToUInt16($bytes,$pe+20)
    $sections=@()
    for($i=0;$i -lt $sectionCount;$i++) {
        $at=$sectionOffset+40*$i
        if($at+40 -gt $bytes.Length){throw 'PE sections exceed image'}
        $sections+=@{virtual=[BitConverter]::ToUInt32($bytes,$at+12);size=[Math]::Max([BitConverter]::ToUInt32($bytes,$at+8),[BitConverter]::ToUInt32($bytes,$at+16));raw=[BitConverter]::ToUInt32($bytes,$at+20)}
    }
    $rvaToOffset={param([uint32]$Rva)
        foreach($section in $sections){if($Rva -ge $section.virtual -and $Rva -lt [uint64]$section.virtual+$section.size){$value=[long]$section.raw+$Rva-$section.virtual;if($value -ge $bytes.Length){throw 'PE RVA exceeds file'};return $value}}
        throw 'PE RVA has no section'
    }
    $importRva=[BitConverter]::ToUInt32($bytes,$optional+120)
    $imports=@()
    if($importRva -ne 0) {
        $at=& $rvaToOffset $importRva
        for($i=0;$i -lt 4096;$i++) {
            if($at+20 -gt $bytes.Length){throw 'PE import table exceeds file'}
            $nameRva=[BitConverter]::ToUInt32($bytes,$at+12)
            if($nameRva -eq 0){break}
            $nameOffset=& $rvaToOffset $nameRva
            $length=0
            while($nameOffset+$length -lt $bytes.Length -and $bytes[$nameOffset+$length] -ne 0 -and $length -lt 512){$length++}
            if($length -eq 512 -or $nameOffset+$length -eq $bytes.Length){throw 'Invalid PE import name'}
            $imports += [Text.Encoding]::ASCII.GetString($bytes,$nameOffset,$length)
            $at+=20
        }
    }
    [pscustomobject]@{architecture='x64';subsystem=[BitConverter]::ToUInt16($bytes,$optional+68);imports=$imports;bytes=$bytes.Length;sha256=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash;version=[Diagnostics.FileVersionInfo]::GetVersionInfo($Path)}
}
