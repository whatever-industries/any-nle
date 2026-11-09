Unicode true
Name "Any NLE"
OutFile "@OUTPUT@"
InstallDir "$LOCALAPPDATA\Programs\Any NLE"
RequestExecutionLevel user
SetCompressor zlib
!include "MUI2.nsh"
!define MUI_ABORTWARNING
!define MUI_ICON "@ICON@"
!define MUI_UNICON "@ICON@"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "@SOURCE@\resources\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Section "Any NLE"
  SetOutPath "$INSTDIR"
  File /r "@SOURCE@\*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Any NLE"
  CreateShortcut "$SMPROGRAMS\Any NLE\Any NLE.lnk" "$INSTDIR\any-nle-gui.exe"
  CreateShortcut "$SMPROGRAMS\Any NLE\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Any NLE" "DisplayName" "Any NLE"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Any NLE" "DisplayIcon" "$INSTDIR\any-nle-gui.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Any NLE" "DisplayVersion" "@VERSION@"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Any NLE" "UninstallString" '"$INSTDIR\Uninstall.exe"'
SectionEnd
Section "Uninstall"
  Delete "$SMPROGRAMS\Any NLE\Any NLE.lnk"
  Delete "$SMPROGRAMS\Any NLE\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Any NLE"
  ; Remove only files shipped by the installer. Never recursively remove user files.
  Delete "$INSTDIR\any-nle.exe"
  Delete "$INSTDIR\any-nle-gui.exe"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\ffprobe.exe"
  Delete "$INSTDIR\backend-manifest.json"
@UNINSTALL_RESOURCES@
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Any NLE"
SectionEnd
