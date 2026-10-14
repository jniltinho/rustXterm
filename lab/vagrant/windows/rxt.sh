#!/usr/bin/env bash
# Host-side driver for rustXterm runs on the Windows test VM (add-windows-port W1 smoke / W3).
# Fictional box account vagrant/vagrant; the VM reaches the host Docker lab at 10.0.2.2.
#   ./rxt.sh push                  copy the Windows build (target/agent-windows release exes) to C:\rxt
#   ./rxt.sh it '<powershell>'     run in the logged-on desktop session (non-elevated), print output
#   ./rxt.sh click X Y [right|double]  mouse click in the desktop session (screen pixels)
#   ./rxt.sh keys 'text'           type into the focused window (VirtualBox keyboard)
#   ./rxt.sh key enter|esc|tab|backspace
#   ./rxt.sh shot NAME             VirtualBox framebuffer screenshot to docs/prints/windows/NAME.png (git-ignored)
set -euo pipefail
self=$(realpath "$0")
cd "$(dirname "$0")"
VM=${RXT_WIN_VM:-rustxterm-windows}
ROOT=../../..
OUT=$ROOT/docs/prints/windows
BUILD=${RXT_WIN_BUILD:-$ROOT/target/agent-windows/x86_64-pc-windows-gnu/release}
gc() { VBoxManage guestcontrol $VM --username vagrant --password vagrant "$@"; }
case "$1" in
  push)
    gc mkdir --parents C:/rxt
    gc copyto --target-directory C:/rxt/ "$(realpath "$BUILD/rustxterm.exe")" "$(realpath "$BUILD/rustxterm-cli.exe")" ;;
  it)
    # One-shot scheduled task in the interactive session; output goes through a file.
    cmd=${2//\'/\'\'}
    vagrant winrm -s powershell -c "
      Remove-Item C:\\rxt\\it.txt -ErrorAction SilentlyContinue
      Set-Content C:\\rxt\\it.ps1 -Value '& { $cmd } *> C:\\rxt\\it.txt'
      \$a = New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File C:\\rxt\\it.ps1'
      \$p = New-ScheduledTaskPrincipal -UserId vagrant -LogonType Interactive -RunLevel Limited
      Register-ScheduledTask -TaskName rxt-it -Action \$a -Principal \$p -Force | Out-Null
      Start-ScheduledTask -TaskName rxt-it
      Start-Sleep -Milliseconds 500
      while ((Get-ScheduledTask -TaskName rxt-it).State -eq 'Running') { Start-Sleep -Milliseconds 200 }
      Get-Content C:\\rxt\\it.txt -ErrorAction SilentlyContinue" ;;
  click)
    n=1; down=2; up=4
    [ "${4:-}" = double ] && n=2
    [ "${4:-}" = right ] && down=8 && up=16
    "$self" it "Add-Type -Name M -Namespace W -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int x, int y); [DllImport(\"user32.dll\")] public static extern void mouse_event(int f, int x, int y, int d, int e); [DllImport(\"user32.dll\")] public static extern bool SetProcessDPIAware();'
      [W.M]::SetProcessDPIAware() | Out-Null
      [W.M]::SetCursorPos($2 - 3, $3) | Out-Null; Start-Sleep -Milliseconds 150
      [W.M]::SetCursorPos($2, $3) | Out-Null; Start-Sleep -Milliseconds 300
      1..$n | ForEach-Object { [W.M]::mouse_event($down,0,0,0,0); [W.M]::mouse_event($up,0,0,0,0); Start-Sleep -Milliseconds 80 }" ;;
  keys)
    # VirtualBox keyboard: no guest process, so the focused rustXterm window keeps the foreground.
    VBoxManage controlvm $VM keyboardputstring "$2" ;;
  key)
    case "$2" in
      enter) sc="1c 9c" ;; esc) sc="01 81" ;; tab) sc="0f 8f" ;; backspace) sc="0e 8e" ;;
      *) echo "key: enter|esc|tab|backspace" >&2; exit 2 ;;
    esac
    # shellcheck disable=SC2086
    VBoxManage controlvm $VM keyboardputscancode $sc ;;
  shot)
    mkdir -p "$OUT"
    sleep 1
    VBoxManage controlvm $VM screenshotpng "$(realpath "$OUT")/$2.png" ;;
  *) echo "usage: $0 push | it '<powershell>' | click X Y [right|double] | keys TEXT | key NAME | shot NAME" >&2; exit 2 ;;
esac
