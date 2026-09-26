#!/bin/sh
# Start a VNC desktop and serve it to the browser.
GEOMETRY=${GEOMETRY:-1440x900}
# A restarted container keeps /tmp, and a stale lock stops Xvnc from starting.
rm -f /tmp/.X1-lock /tmp/.X11-unix/X1
Xvnc :1 -geometry "$GEOMETRY" -depth 24 -SecurityTypes None -localhost yes &
sleep 1
export DISPLAY=:1
dbus-launch startxfce4 >/tmp/xfce.log 2>&1 &
exec websockify --web /usr/share/novnc 6080 localhost:5901
