!include "MUI2.nsh"

Name "Local Notebook Agent"
OutFile "..\dist\LocalNotebook-Setup-win-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\LocalNotebook"
InstallDirRegKey HKCU "Software\LocalNotebookAgent" "InstallDir"
RequestExecutionLevel user
Unicode True

!define MUI_ABORTWARNING

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

Section "MainSection" SEC01
  SetOutPath "$INSTDIR"
  File /r "..\dist\LocalNotebook-portable\*.*"
  WriteRegStr HKCU "Software\LocalNotebookAgent" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  CreateDirectory "$SMPROGRAMS\Local Notebook Agent"
  CreateShortcut "$SMPROGRAMS\Local Notebook Agent\Local Notebook Agent.lnk" "$INSTDIR\LocalNotebook.exe"
  CreateShortcut "$SMPROGRAMS\Local Notebook Agent\卸载 Local Notebook Agent.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\Local Notebook Agent.lnk" "$INSTDIR\LocalNotebook.exe"
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\Local Notebook Agent.lnk"
  RMDir /r "$SMPROGRAMS\Local Notebook Agent"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\LocalNotebookAgent"
SectionEnd
