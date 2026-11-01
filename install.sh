#!/bin/bash
# Aero7 installer for Arch / EndeavourOS.
#   ./install.sh [/path/to/your/windows7.iso]
set -e
cd "$(dirname "$0")"
DEPS=(gtk3 python cairo pango libx11 libxcb xorg-xsetroot xsettingsd picom
      7zip wimlib papirus-icon-theme noto-fonts i3lock xss-lock polkit
      pipewire-pulse wireplumber pavucontrol networkmanager brightnessctl
      playerctl udiskie power-profiles-daemon gnome-keyring rust)
echo ":: installing dependencies (sudo)"
sudo pacman -S --needed --noconfirm "${DEPS[@]}"
echo ":: building"
cargo build --release
echo ":: installing to /usr (sudo)"
for b in aero7 aero7-wm aero7-extract aero7-polkit; do sudo install -Dm755 "target/release/$b" "/usr/bin/$b"; done
for b in target/release/aero7-*; do [ -x "$b" ] && [ -f "$b" ] && sudo install -Dm755 "$b" "/usr/bin/$(basename "$b")"; done
sudo install -Dm755 data/aero7-session /usr/bin/aero7-session
sudo install -Dm644 data/picom.conf /usr/share/aero7/picom.conf
sudo install -Dm644 data/gtk.css /usr/share/themes/Aero7/gtk-3.0/gtk.css
sudo install -Dm644 data/aero7.desktop /usr/share/xsessions/aero7.desktop
for f in data/applications/*.desktop; do [ -f "$f" ] && sudo install -Dm644 "$f" "/usr/share/applications/$(basename "$f")"; done
if [ -n "$1" ]; then
  echo ":: extracting Windows 7 artwork from $1 (stays in your home directory)"
  aero7-extract "$1"
else
  echo ":: no ISO given – run 'aero7-extract /path/to/windows7.iso' later for the real Win7 look"
fi
echo ":: done. Log out and pick the 'Aero7' session in your login manager."
