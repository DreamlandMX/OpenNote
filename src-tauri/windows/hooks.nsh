; OpenNote NSIS installer hooks (Tauri 2 `bundle.windows.nsis.installerHooks`).

; Before any file is removed: revoke the LaTeX sandbox's approved
; profile-folder entries (FILE_READ_ATTRIBUTES on %USERPROFILE% and %APPDATA%),
; delete its AppContainer profiles and remove its runtime directory.
!macro NSIS_HOOK_PREUNINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --latex-uninstall-cleanup'
!macroend
