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
OutFile "..\Tonearm-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Tonearm"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
BrandingText "${NAME} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Free software under the GNU GPL v3"

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
; The finish page's second tick box makes a desktop shortcut. It is off at
; first, and a silent update never shows it, so an update leaves the desktop
; as it was.
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Create a desktop shortcut"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateDesktopShortcut

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

; Asks a running Tonearm to quit, then waits up to about 15 seconds for it to
; let go of its exe. The updater closes the app before it starts the
; installer, but a copy started by hand may still be open, often hidden in
; the tray. If the exe is still locked after that, asks the user to quit it,
; and stops if they cancel.
!macro CloseTonearm path
  ; ${__LINE__} changes on every line of a macro, so take one id for all the
  ; labels of this use of it.
  !define CT_ID ${__LINE__}
  ; Copies from 0.6.1 on quit when they get this message.
  System::Call 'user32::RegisterWindowMessage(t "Tonearm.InstallerQuit") i .r2'
  System::Call 'user32::PostMessage(p 0xffff, i r2, p 0, p 0)'
  StrCpy $0 0
  close_check_${CT_ID}:
    IfFileExists "${path}" 0 close_done_${CT_ID}
    ClearErrors
    FileOpen $1 "${path}" a
    IfErrors 0 close_free_${CT_ID}
    IntOp $0 $0 + 1
    IntCmp $0 30 close_ask_${CT_ID} 0 close_ask_${CT_ID}
    Sleep 500
    Goto close_check_${CT_ID}
  close_ask_${CT_ID}:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Tonearm is still running.$\r$\n$\r$\nQuit it from its tray icon (right click it, then Quit), then click Retry." /SD IDCANCEL IDRETRY close_retry_${CT_ID}
    Abort
  close_retry_${CT_ID}:
    ; A few more seconds to let go after it quits.
    StrCpy $0 24
    Goto close_check_${CT_ID}
  close_free_${CT_ID}:
    FileClose $1
  close_done_${CT_ID}:
  !undef CT_ID
!macroend

Section "Tonearm"
  SetOutPath "$INSTDIR"
  !insertmacro CloseTonearm "$INSTDIR\tonearm.exe"
  File "/oname=tonearm.exe" "${EXE}"
  File "/oname=THIRD_PARTY_NOTICES.txt" "..\THIRD_PARTY_NOTICES.md"
  ; Left over from the exe swapping updater of 0.5 and before.
  Delete "$INSTDIR\tonearm.old"
  Delete "$INSTDIR\tonearm.new"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Tonearm keeps this shortcut's app ID up to date itself, for the media
  ; flyout, so it only needs to exist here.
  CreateShortcut "$SMPROGRAMS\Tonearm.lnk" "$INSTDIR\tonearm.exe"
  ; A desktop shortcut made at an earlier install is written again, so the
  ; shell reads it afresh.
  IfFileExists "$DESKTOP\Tonearm.lnk" 0 +2
    CreateShortcut "$DESKTOP\Tonearm.lnk" "$INSTDIR\tonearm.exe"
  Call RefreshIcons

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

Function CreateDesktopShortcut
  CreateShortcut "$DESKTOP\Tonearm.lnk" "$INSTDIR\tonearm.exe"
  Call RefreshIcons
FunctionEnd

; Windows keeps a cache of icons, so an update that changes the logo would
; leave the old one on the Start menu, the desktop and the taskbar pin, which
; all take it from the exe. Tells the shell each of those changed, then that
; icons changed everywhere, and has Windows rebuild its icon cache.
!define SHCNE_UPDATEITEM 0x2000
!define SHCNE_ASSOCCHANGED 0x08000000
!define SHCNF_PATHW_FLUSH 0x1005
!define SHCNF_IDLIST_FLUSH 0x1000
!define PINNED "$APPDATA\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\Tonearm.lnk"
Function RefreshIcons
  System::Call 'shell32::SHChangeNotify(i ${SHCNE_UPDATEITEM}, i ${SHCNF_PATHW_FLUSH}, w "$INSTDIR\tonearm.exe", p 0)'
  System::Call 'shell32::SHChangeNotify(i ${SHCNE_UPDATEITEM}, i ${SHCNF_PATHW_FLUSH}, w "$SMPROGRAMS\Tonearm.lnk", p 0)'
  IfFileExists "$DESKTOP\Tonearm.lnk" 0 +2
    System::Call 'shell32::SHChangeNotify(i ${SHCNE_UPDATEITEM}, i ${SHCNF_PATHW_FLUSH}, w "$DESKTOP\Tonearm.lnk", p 0)'
  IfFileExists "${PINNED}" 0 +2
    System::Call 'shell32::SHChangeNotify(i ${SHCNE_UPDATEITEM}, i ${SHCNF_PATHW_FLUSH}, w "${PINNED}", p 0)'
  System::Call 'shell32::SHChangeNotify(i ${SHCNE_ASSOCCHANGED}, i ${SHCNF_IDLIST_FLUSH}, p 0, p 0)'
  ; Windows 10 and 11 refresh their icon cache with this, quietly. The
  ; installer is 32 bit, so the 64 bit system folder is reached through
  ; Sysnative. It does not matter if neither is there.
  StrCpy $0 "$WINDIR\Sysnative\ie4uinit.exe"
  IfFileExists $0 refresh_run 0
  StrCpy $0 "$SYSDIR\ie4uinit.exe"
  IfFileExists $0 refresh_run refresh_done
  refresh_run:
    nsExec::Exec '"$0" -show'
    Pop $0
  refresh_done:
FunctionEnd

Section "Uninstall"
  !insertmacro CloseTonearm "$INSTDIR\tonearm.exe"
  Delete "$INSTDIR\tonearm.exe"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.txt"
  Delete "$INSTDIR\tonearm.old"
  Delete "$INSTDIR\tonearm.new"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Tonearm.lnk"
  Delete "$DESKTOP\Tonearm.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  ; Settings, sign in, history and downloads stay, in case Tonearm comes
  ; back. They live in the user's AppData, not here.
SectionEnd
