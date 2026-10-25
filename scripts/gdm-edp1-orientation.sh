#!/bin/bash
# 登录界面读 /var/lib/gdm3/seat0/config/monitors.xml。
# 主屏倒装由内核 video=eDP-1:panel_orientation=upside_down 报告，
# 这里的逻辑方向必须是 normal，不能再写 upside_down。
# 用户会话开了分数缩放。登录界面没有这项时，带缩放的配置会被整份丢掉。
set -euo pipefail

if [[ ${EUID} -ne 0 ]]; then
  echo "请用 sudo bash $0" >&2
  exit 1
fi

user=${SUDO_USER:-cwang}
src=/home/${user}/.config/monitors.xml
dest=/var/lib/gdm3/seat0/config/monitors.xml
defaults=/etc/gdm3/greeter.dconf-defaults

[[ -f $src ]] || { echo "找不到 $src" >&2; exit 1; }
[[ -d /var/lib/gdm3/seat0/config ]] || { echo "找不到 GDM seat0 配置目录" >&2; exit 1; }

install -m 644 "$src" "$dest"
sed -i 's/<rotation>upside_down<\/rotation>/<rotation>normal<\/rotation>/' "$dest"

if ! grep -q 'scale-monitor-framebuffer' "$defaults"; then
  cat >> "$defaults" <<'EOF'

[org/gnome/mutter]
experimental-features=['scale-monitor-framebuffer', 'xwayland-native-scaling']
EOF
fi

/usr/share/gdm/generate-config
echo "已写入 $dest"
