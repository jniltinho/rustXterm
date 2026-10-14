# In-VM input helper for Wayland and X11 sessions (sourced): y <ydotool args>; to X Y; click X Y;
# drag X1 Y1 X2 Y2. Needs the daemon, started once per boot:
#   sudo ydotoold --socket-path=/tmp/.ydotool_socket --socket-own="$(id -u):$(id -g)" &
# GNOME: `gsettings set org.gnome.desktop.peripherals.mouse accel-profile flat` and
# `... interface enable-hot-corners false`. KDE Wayland: REL=1 and a flat profile on the
# "ydotoold virtual device" (org.kde.KWin InputDevice pointerAccelerationProfileFlat). On Xorg
# sessions use xdotool instead (ydotool's relative moves are accelerated there).
export YDOTOOL_SOCKET=/tmp/.ydotool_socket
y() { ydotool "$@" >/dev/null; }
to() { if [ -n "${REL:-}" ]; then ydotool mousemove -x -4000 -y -4000 >/dev/null; sleep 0.15; ydotool mousemove -x "$1" -y "$2" >/dev/null; else ydotool mousemove --absolute -x "$1" -y "$2" >/dev/null; fi; sleep 0.2; }
click() { to "$1" "$2"; ydotool click 0xC0 >/dev/null; sleep 0.5; }
drag() { to "$1" "$2"; ydotool click 0x40 >/dev/null; sleep 0.2; ydotool mousemove -x 10 -y 0 >/dev/null; sleep 0.1; ydotool mousemove -x 10 -y 0 >/dev/null; sleep 0.1; to "$3" "$4"; ydotool click 0x80 >/dev/null; sleep 0.5; }
