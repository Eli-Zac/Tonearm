; Tonearm's installer. Installs for the current user only, so it needs no
; admin rights and the in-app updater can run it silently.
;
;   makensis /DVERSION=0.6.0 installer\tonearm.nsi
;
; Paths are relative to this folder. The pictures come from
; assets/render_installer.py.
;
; The updater runs it as `tonearm-setup.exe /S /relaunch`. /S installs with
; no pages, and /relaunch starts Tonearm again once it is done.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "..\target\release\tonearm.exe"
!endif

!define NAME "Tonearm"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tonearm"

!include "MUI2.nsh"
!include "FileFunc.nsh"

Name "${NAME}"
OutFile "..\tonearm-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Tonearm"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
BrandingText "${NAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!define MUI_ICON "..\assets\tonearm.ico"
!define MUI_UNICON "..\assets\tonearm.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEFINISHPAGE_BITMAP "..\assets\installer-side.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "..\assets\installer-side.bmp"
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "..\assets\installer-header.bmp"
!define MUI_HEADERIMAGE_UNBITMAP "..\assets\installer-header.bmp"
!define MUI_WELCOMEPAGE_TITLE "Welcome to Tonearm"
!define MUI_WELCOMEPAGE_TEXT "A native Windows player for YouTube Music.$\r$\n$\r$\nTonearm installs just for you, so it needs no admin rights. Your settings and songs stay if you install over an older version.$\r$\n$\r$\nTonearm is unofficial and not affiliated with Google or YouTube.$\r$\n$\r$\nClick Next to continue."
!define MUI_FINISHPAGE_TITLE "Tonearm is installed"
!define MUI_FINISHPAGE_TEXT "You can find Tonearm in the Start menu. Closing its window keeps the music playing in the tray, which Settings can change."
!define MUI_FINISHPAGE_RUN "$INSTDIR\tonearm.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Tonearm"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

; Waits up to about 15 seconds for a running Tonearm to let go of its exe.
; The updater closes the app just before it starts the installer.
!macro WaitForExe path
  StrCpy $0 0
  wait_loop_${__LINE__}:
    IfFileExists "${path}" 0 wait_done_${__LINE__}
    ClearErrors
    FileOpen $1 "${path}" a
    IfErrors 0 wait_open_${__LINE__}
    IntOp $0 $0 + 1
    IntCmp $0 30 wait_done_${__LINE__}
    Sleep 500
    Goto wait_loop_${__LINE__}
  wait_open_${__LINE__}:
    FileClose $1
  wait_done_${__LINE__}:
!macroend

Section "Tonearm"
  SetOutPath "$INSTDIR"
  !insertmacro WaitForExe "$INSTDIR\tonearm.exe"
  ClearErrors
  File "/oname=tonearm.exe" "${EXE}"
  IfErrors 0 copied
    MessageBox MB_ICONSTOP "Tonearm is still running. Close it and try again." /SD IDOK
    Abort
  copied:
  ; Left over from the exe swapping updater of 0.5 and before.
  Delete "$INSTDIR\tonearm.old"
  Delete "$INSTDIR\tonearm.new"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Tonearm keeps this shortcut's app ID up to date itself, for the media
  ; flyout, so it only needs to exist here.
  CreateShortcut "$SMPROGRAMS\Tonearm.lnk" "$INSTDIR\tonearm.exe"

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\tonearm.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0

  ; An update starts the app again when it is done.
  ${GetParameters} $0
  ${GetOptions} $0 "/relaunch" $1
  IfErrors +2 0
    Exec '"$INSTDIR\tonearm.exe"'
SectionEnd

Section "Uninstall"
  !insertmacro WaitForExe "$INSTDIR\tonearm.exe"
  Delete "$INSTDIR\tonearm.exe"
  Delete "$INSTDIR\tonearm.old"
  Delete "$INSTDIR\tonearm.new"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Tonearm.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Settings, sign in, history and downloads stay, in case Tonearm comes
  ; back. They live in the user's AppData, not here.
SectionEnd
