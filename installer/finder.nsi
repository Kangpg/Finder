# -*- coding: utf-8 -*-
Unicode true
!include "MUI2.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "LogicLib.nsh"
!ifndef PAYLOAD_DIR
  !error "Use scripts/build-release.ps1 to supply the verified release payload"
!endif
Name "${PRODUCT_NAME}"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${PUBLISHER}\${PRODUCT_NAME}"
InstallDirRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetDatablockOptimize on
BrandingText "${PUBLISHER}"
Icon "${PAYLOAD_DIR}\finder.ico"
UninstallIcon "${PAYLOAD_DIR}\finder.ico"
VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1033 "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=1033 "FileDescription" "${PRODUCT_NAME} Setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${PRODUCT_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (c) ${PUBLISHER}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\finder.exe"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "$(DataPreserved)"
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Korean"
!insertmacro MUI_LANGUAGE "English"
LangString DesktopShortcut ${LANG_KOREAN} "바탕 화면 바로 가기"
LangString DesktopShortcut ${LANG_ENGLISH} "Desktop shortcut"
LangString PlatformRequired ${LANG_KOREAN} "Finder는 Windows 10 이상 64비트 환경이 필요합니다."
LangString PlatformRequired ${LANG_ENGLISH} "Finder requires 64-bit Windows 10 or later."
LangString UnknownDirectory ${LANG_KOREAN} "Finder가 소유한 설치 폴더인지 확인할 수 없습니다. 다른 폴더를 선택해 주세요."
LangString UnknownDirectory ${LANG_ENGLISH} "This folder cannot be verified as a Finder installation. Choose another folder."
LangString DataPreserved ${LANG_KOREAN} "개인 설정과 색인은 유지됩니다."
LangString DataPreserved ${LANG_ENGLISH} "Personal settings and indexes are preserved."
LangString FilesInUse ${LANG_KOREAN} "Finder 파일이 사용 중이거나 수정 권한이 없습니다. Finder를 완전히 종료한 뒤 다시 시도해 주세요."
LangString FilesInUse ${LANG_ENGLISH} "Finder files are in use or cannot be modified. Exit Finder completely and try again."
Var OwnsIntegration
Var OwnsStartMenu
Var OwnsDesktop
!macro CheckWritableExecutable FilePath
  ${If} ${FileExists} "${FilePath}"
    System::Call 'kernel32::CreateFileW(w "${FilePath}", i 0x40000000, i 0, p 0, i 3, i 0x80, p 0) p .r0'
    ${If} $0 == -1
      MessageBox MB_ICONSTOP "$(FilesInUse)" /SD IDOK
      Abort
    ${EndIf}
    System::Call 'kernel32::CloseHandle(p r0)'
  ${EndIf}
!macroend
Function .onInit
  SetShellVarContext current
  SetRegView 64
  System::Call 'advapi32::RegOpenKeyExW(p 0x80000001, w "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}", i 0, i 0x20119, *p.r0) i.r1'
  ${If} $1 == 0
    System::Call 'advapi32::RegCloseKey(p r0)'
    ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "ProductId"
    ${If} $0 != "${PRODUCT_ID}"
      MessageBox MB_ICONSTOP "$(UnknownDirectory)" /SD IDOK
      Abort
    ${EndIf}
  ${ElseIf} $1 != 2
    MessageBox MB_ICONSTOP "$(UnknownDirectory)" /SD IDOK
    Abort
  ${EndIf}
  !insertmacro MUI_LANGDLL_DISPLAY
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "$(PlatformRequired)" /SD IDOK
    Abort
  ${EndIf}
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "$(PlatformRequired)" /SD IDOK
    Abort
  ${EndIf}
FunctionEnd
Section "${PRODUCT_NAME}" MainSection
  SectionIn RO
  IfFileExists "$INSTDIR\${INSTALL_MARKER}" verify_identity check_new_folder
verify_identity:
  ClearErrors
  FileOpen $0 "$INSTDIR\${INSTALL_MARKER}" r
  IfErrors foreign_directory
  FileRead $0 $1
  FileClose $0
  StrCmp $1 "${PRODUCT_ID}" verified_folder foreign_directory
check_new_folder:
  IfFileExists "$INSTDIR\finder.exe" foreign_directory
  IfFileExists "$INSTDIR\finder-cli.exe" foreign_directory
  IfFileExists "$INSTDIR\README.md" foreign_directory
  IfFileExists "$INSTDIR\THIRD-PARTY-NOTICES.txt" foreign_directory
  IfFileExists "$INSTDIR\Uninstall.exe" foreign_directory verified_folder
foreign_directory:
  MessageBox MB_ICONSTOP "$(UnknownDirectory)" /SD IDOK
  Abort
verified_folder:
  !insertmacro CheckWritableExecutable "$INSTDIR\finder.exe"
  !insertmacro CheckWritableExecutable "$INSTDIR\finder-cli.exe"
  StrCpy $OwnsStartMenu 0
  StrCpy $OwnsDesktop 0
  ReadRegDWORD $OwnsStartMenu HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "StartMenuShortcutOwned"
  ReadRegDWORD $OwnsDesktop HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DesktopShortcutOwned"
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File "${PAYLOAD_DIR}\finder.exe"
  File "${PAYLOAD_DIR}\finder-cli.exe"
  File "${PAYLOAD_DIR}\README.md"
  File "${PAYLOAD_DIR}\THIRD-PARTY-NOTICES.txt"
  File "${PAYLOAD_DIR}\${INSTALL_MARKER}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\${PUBLISHER}"
  ${IfNot} ${FileExists} "$SMPROGRAMS\${PUBLISHER}\${PRODUCT_NAME}.lnk"
    CreateShortcut "$SMPROGRAMS\${PUBLISHER}\${PRODUCT_NAME}.lnk" "$INSTDIR\finder.exe"
    StrCpy $OwnsStartMenu 1
  ${ElseIf} $OwnsStartMenu == 1
    CreateShortcut "$SMPROGRAMS\${PUBLISHER}\${PRODUCT_NAME}.lnk" "$INSTDIR\finder.exe"
  ${EndIf}
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "ProductId" "${PRODUCT_ID}"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "InstallerLanguage" $LANGUAGE
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DisplayIcon" '"$INSTDIR\finder.exe",0'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "StartMenuShortcutOwned" $OwnsStartMenu
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DesktopShortcutOwned" $OwnsDesktop
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "NoRepair" 1
SectionEnd
Section /o "$(DesktopShortcut)" DesktopSection
  ${IfNot} ${FileExists} "$DESKTOP\${PRODUCT_NAME}.lnk"
    CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\finder.exe"
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DesktopShortcutOwned" 1
  ${ElseIf} $OwnsDesktop == 1
    CreateShortcut "$DESKTOP\${PRODUCT_NAME}.lnk" "$INSTDIR\finder.exe"
  ${EndIf}
SectionEnd
Function un.onInit
  SetShellVarContext current
  SetRegView 64
  ReadRegDWORD $LANGUAGE HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "InstallerLanguage"
  ClearErrors
  FileOpen $0 "$INSTDIR\${INSTALL_MARKER}" r
  IfErrors un_foreign_directory
  FileRead $0 $1
  FileClose $0
  StrCmp $1 "${PRODUCT_ID}" un_verified_folder un_foreign_directory
un_foreign_directory:
  MessageBox MB_ICONSTOP "$(UnknownDirectory)" /SD IDOK
  Abort
un_verified_folder:
  !insertmacro CheckWritableExecutable "$INSTDIR\finder.exe"
  !insertmacro CheckWritableExecutable "$INSTDIR\finder-cli.exe"
  ReadRegDWORD $OwnsStartMenu HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "StartMenuShortcutOwned"
  ReadRegDWORD $OwnsDesktop HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "DesktopShortcutOwned"
  StrCpy $OwnsIntegration 0
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "InstallLocation"
  ${If} $0 == $INSTDIR
    ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}" "ProductId"
    ${If} $0 == "${PRODUCT_ID}"
      StrCpy $OwnsIntegration 1
    ${EndIf}
  ${EndIf}
FunctionEnd
Section "Uninstall"
  Delete "$INSTDIR\finder.exe"
  Delete "$INSTDIR\finder-cli.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
  Delete "$INSTDIR\${INSTALL_MARKER}"
  Delete "$INSTDIR\Uninstall.exe"
  ${If} $OwnsIntegration == 1
    ${If} $OwnsStartMenu == 1
      Delete "$SMPROGRAMS\${PUBLISHER}\${PRODUCT_NAME}.lnk"
      RMDir "$SMPROGRAMS\${PUBLISHER}"
    ${EndIf}
    ${If} $OwnsDesktop == 1
      Delete "$DESKTOP\${PRODUCT_NAME}.lnk"
    ${EndIf}
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_ID}"
  ${EndIf}
  ; No recursive removal: portable leftovers and profile data belong to the user.
  RMDir "$INSTDIR"
SectionEnd
