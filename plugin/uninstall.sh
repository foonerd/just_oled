#!/bin/bash
# Just OLED uninstaller: everything install.sh and the plugin put outside
# the plugin folder goes; the plugin folder itself the plugin manager
# removes.

echo "Uninstalling Just OLED"

PLUGIN_DIR="/data/plugins/system_hardware/just_oled"
SERVICE="just-oled"
UNIT="/etc/systemd/system/$SERVICE.service"
SUDOERS="/etc/sudoers.d/volumio-user-just_oled"
USERCONFIG="/boot/userconfig.txt"
MARK_BEGIN="# >>> just_oled"
MARK_END="# <<< just_oled"

systemctl stop "$SERVICE.service" 2>/dev/null || true
systemctl disable "$SERVICE.service" 2>/dev/null || true
rm -f "$UNIT"
systemctl daemon-reload

rm -f "$SUDOERS"
rm -f /etc/udev/rules.d/60-just-oled.rules /etc/modules-load.d/just-oled.conf
udevadm control --reload 2>/dev/null || true
rm -f /tmp/just-oled.sock /dev/shm/justoled.*
rm -f "$PLUGIN_DIR/alsa-lib/libjustoledtap.so"

# The lines the plugin wrote into the boot configuration, between its marks.
if [ -f "$USERCONFIG" ] && grep -q "^$MARK_BEGIN" "$USERCONFIG"; then
  sed -i "/^$MARK_BEGIN/,/^$MARK_END/d" "$USERCONFIG"
  echo "Removed the plugin's lines from $USERCONFIG"
fi

echo "Just OLED uninstalled"
echo "pluginuninstallend"
