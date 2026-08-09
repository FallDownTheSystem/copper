; NSIS installer hooks for shipping copper-cli on the machine PATH.
;
; Tauri's bundler already copies the externalBin file to $INSTDIR\copper-cli.exe
; earlier in `Section Install` (the "Copy external binaries" step) and already
; deletes it from the $INSTDIR root on uninstall (the "Delete external binaries"
; step). Both are stock, unconditional parts of the generated installer.nsi. By
; the time either hook below runs, our own Rename or Delete has already moved
; that file out of the way, so those stock steps find nothing and no-op
; harmlessly. Authenticode signing of the sidecar is not one of those steps —
; tauri-bundler signs the staged file before makensis ever runs, so what NSIS
; packages here is already signed.
;
; PATH itself is read and written by copper-path.ps1, sitting next to this file,
; not by NSIS string macros. The NSIS build Tauri downloads reports
; NSIS_MAX_STRLEN=1024, and a machine PATH routinely exceeds that: ReadRegStr
; would truncate the value silently and writing it back would destroy the tail
; of the user's PATH.

; ${__FILEDIR__} has to be captured HERE, at include time, and not used inside
; the macro bodies below. NSIS stores a macro as raw lines and preprocesses them
; where the macro is inserted, so a ${__FILEDIR__} inside a macro body expands to
; the directory of the *generated* installer.nsi that inserts it — which is a
; bundler temp directory, with no copper-path.ps1 in it. Verified by compiling
; both spellings with the bundled makensis; the naive one fails with
; "no files found".
!define COPPER_HOOKS_DIR "${__FILEDIR__}"

!macro NSIS_HOOK_POSTINSTALL
	; $INSTDIR already holds the GUI's own ${MAINBINARYNAME}.exe — "copper.exe",
	; lowercase, derived from the Cargo package name rather than from
	; productName — so the CLI cannot take that name at the $INSTDIR root under
	; any scheme. It goes in a subdirectory instead, which also means only that
	; subdirectory ever goes on PATH: a bare `copper` in a terminal can never
	; resolve to the GUI binary.
	ClearErrors
	CreateDirectory "$INSTDIR\cli"
	${IfThen} ${Errors} ${|} DetailPrint "copper: could not create $INSTDIR\cli" ${|}

	; On an update or a re-run, $INSTDIR\cli\copper.exe is already there from the
	; previous install, and NSIS's Rename fails when the destination exists —
	; which would leave the OLD CLI in place and silently defeat the update.
	;
	; The old binary is stepped aside rather than deleted outright, so a failed
	; replacement can be undone: deleting first and then failing the Rename would
	; leave an update with no CLI at all, which is worse than an out-of-date one.
	; Neither of these two can usefully fail on a first install (there is nothing
	; to move), so only the replacement itself is checked.
	ClearErrors
	Delete "$INSTDIR\cli\copper.exe.old"
	Rename "$INSTDIR\cli\copper.exe" "$INSTDIR\cli\copper.exe.old"

	ClearErrors
	Rename "$INSTDIR\copper-cli.exe" "$INSTDIR\cli\copper.exe"
	${If} ${Errors}
		DetailPrint "copper: could not move the CLI to $INSTDIR\cli\copper.exe"
		ClearErrors
		Rename "$INSTDIR\cli\copper.exe.old" "$INSTDIR\cli\copper.exe"
		${IfThen} ${Errors} ${|} DetailPrint "copper: and the previous CLI could not be put back; there is no copper.exe in $INSTDIR\cli" ${|}
	${Else}
		Delete "$INSTDIR\cli\copper.exe.old"
	${EndIf}

	; Only put the directory on PATH once something is actually in it. A PATH
	; entry pointing at a directory with no copper.exe buys nothing and has to be
	; removed by hand later.
	${If} ${FileExists} "$INSTDIR\cli\copper.exe"
		; $PLUGINSDIR is only guaranteed to exist after InitPluginsDir, and
		; `File` needs it to exist before it can extract into it.
		InitPluginsDir
		File "/oname=$PLUGINSDIR\copper-path.ps1" "${COPPER_HOOKS_DIR}\copper-path.ps1"
		nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\copper-path.ps1" -Action Add -Dir "$INSTDIR\cli"'
		Pop $0
		${IfThen} $0 != 0 ${|} DetailPrint "copper: could not add $INSTDIR\cli to the machine PATH (PowerShell exit $0); run the installer again or add it by hand" ${|}
	${Else}
		DetailPrint "copper: no CLI landed in $INSTDIR\cli, so the machine PATH was left alone"
	${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
	; PATH first, files second. If the PATH edit fails the files still go — an
	; uninstall that leaves binaries behind is worse than one that leaves a dead
	; PATH entry, and a dead entry costs nothing but a wasted directory lookup.
	; It is reported here and recorded in doc-release-process.md's known
	; limitations rather than silently accepted.
	InitPluginsDir
	File "/oname=$PLUGINSDIR\copper-path.ps1" "${COPPER_HOOKS_DIR}\copper-path.ps1"
	nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\copper-path.ps1" -Action Remove -Dir "$INSTDIR\cli"'
	Pop $0
	${IfThen} $0 != 0 ${|} DetailPrint "copper: could not remove $INSTDIR\cli from the machine PATH (PowerShell exit $0); remove it by hand" ${|}

	ClearErrors
	Delete "$INSTDIR\cli\copper.exe"
	${IfThen} ${Errors} ${|} DetailPrint "copper: could not delete $INSTDIR\cli\copper.exe" ${|}

	; A leftover from an install whose replacement failed and whose restore then
	; also failed. Rare, but RMDir below is non-recursive and would refuse the
	; directory if one were sitting in it.
	Delete "$INSTDIR\cli\copper.exe.old"

	ClearErrors
	RMDir "$INSTDIR\cli"
	${IfThen} ${Errors} ${|} DetailPrint "copper: could not remove the $INSTDIR\cli directory" ${|}

	; Section Uninstall's own `RMDir "$INSTDIR"` runs BEFORE this hook and is
	; non-recursive, so with cli\ still present it silently failed to remove
	; $INSTDIR — the generated uninstaller has no knowledge of a directory it
	; did not create. Retry now that cli\ is gone, so a plain uninstall does not
	; leave an empty %ProgramFiles%\Copper behind.
	ClearErrors
	RMDir "$INSTDIR"
	${IfThen} ${Errors} ${|} DetailPrint "copper: $INSTDIR was left in place (still in use, or holding files the installer did not put there)" ${|}
!macroend
