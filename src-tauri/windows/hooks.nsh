; Remove the per-user "Launch at startup" registration (written by the autostart plugin) on uninstall,
; so no stale Run entry points at a deleted executable.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Kaiwen Notify"
!macroend
