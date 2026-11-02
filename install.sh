#!/bin/bash
# Aerode installer for Arch / EndeavourOS.
#   ./install.sh [/path/to/your/windows7.iso]
set -e
cd "$(dirname "$0")"
DEPS=(gtk3 python cairo pango libx11 libxcb xorg-xsetroot xsettingsd picom
      7zip wimlib papirus-icon-theme noto-fonts i3lock xss-lock polkit
      pipewire-pulse wireplumber pavucontrol networkmanager brightnessctl
      playerctl ddcutil udiskie power-profiles-daemon rust)
# KDE users already have KWallet; a second keyring hijacks org.freedesktop.secrets and logs apps out
pacman -Q kwallet >/dev/null 2>&1 || DEPS+=(gnome-keyring)
echo ":: installing dependencies (sudo)"
sudo pacman -S --needed --noconfirm "${DEPS[@]}"
echo ":: enabling DDC/CI (external monitor brightness)"
echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf >/dev/null
sudo modprobe i2c-dev || true
getent group i2c >/dev/null && sudo usermod -aG i2c "$USER" || true
echo ":: migrating old aero7 files (if any)"
for pair in ".local/share/aero7:.local/share/aerode" ".config/aero7:.config/aerode" ".local/share/icons/Aero7:.local/share/icons/Aerode" ".local/share/icons/Aero7-cursors:.local/share/icons/Aerode-cursors" ".local/share/fonts/aero7:.local/share/fonts/aerode"; do
  old="$HOME/${pair%%:*}"; new="$HOME/${pair##*:}"
  [ -e "$old" ] && [ ! -e "$new" ] && mv "$old" "$new"
done
sed -i 's/Aero7/Aerode/g' "$HOME/.local/share/icons/Aerode/index.theme" "$HOME/.local/share/icons/Aerode-cursors/index.theme" 2>/dev/null || true
for b in aero7 aero7-wm aero7-extract aero7-polkit aero7-cmd aero7-explorer aero7-control aero7-notepad aero7-calc aero7-taskmgr aero7-snip aero7-session; do sudo rm -f "/usr/bin/$b"; done
sudo rm -f /usr/share/xsessions/aero7.desktop /usr/share/applications/aero7-*.desktop
echo ":: building"
cargo build --release
echo ":: installing to /usr (sudo)"
for b in aerode aerode-wm aerode-extract aerode-polkit; do sudo install -Dm755 "target/release/$b" "/usr/bin/$b"; done
for b in target/release/aerode-*; do [ -x "$b" ] && [ -f "$b" ] && sudo install -Dm755 "$b" "/usr/bin/$(basename "$b")"; done
sudo install -Dm755 data/aerode-session /usr/bin/aerode-session
sudo install -Dm644 data/picom.conf /usr/share/aerode/picom.conf
sudo install -Dm644 data/gtk.css /usr/share/themes/Aerode/gtk-3.0/gtk.css
sudo install -Dm644 data/aerode.desktop /usr/share/xsessions/aerode.desktop
for f in data/applications/*.desktop; do [ -f "$f" ] && sudo install -Dm644 "$f" "/usr/share/applications/$(basename "$f")"; done
if [ -n "$1" ]; then
  echo ":: extracting Windows 7 artwork from $1 (stays in your home directory)"
  aerode-extract "$1"
else
  echo ":: no ISO given – run 'aerode-extract /path/to/windows7.iso' later for the real Win7 look"
fi
echo ":: done. Log out and pick the 'Aerode' session in your login manager."
