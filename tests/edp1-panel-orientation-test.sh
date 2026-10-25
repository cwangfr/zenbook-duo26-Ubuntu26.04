#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT_DIR}/tools/configure-edp1-panel-orientation.sh"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TEMP_DIR}"' EXIT

mkdir -p "${TEMP_DIR}/bin" "${TEMP_DIR}/home/.config" "${TEMP_DIR}/gdm"
printf 'ASUS\n' > "${TEMP_DIR}/vendor"
printf 'Zenbook Duo UX8407AA\n' > "${TEMP_DIR}/product"
printf 'quiet splash\n' > "${TEMP_DIR}/cmdline"
cat > "${TEMP_DIR}/home/.config/monitors.xml" <<'EOF'
<transform>
  <rotation>upside_down</rotation>
</transform>
EOF
cp "${TEMP_DIR}/home/.config/monitors.xml" "${TEMP_DIR}/gdm/monitors.xml"

cat > "${TEMP_DIR}/bin/sudo" <<'EOF'
#!/usr/bin/env bash
exec "$@"
EOF
cat > "${TEMP_DIR}/bin/update-grub" <<'EOF'
#!/usr/bin/env bash
printf 'updated\n' >> "${UPDATE_LOG}"
EOF
chmod +x "${TEMP_DIR}/bin/sudo" "${TEMP_DIR}/bin/update-grub" "${SCRIPT}"

export PATH="${TEMP_DIR}/bin:/usr/bin:/bin"
export HOME="${TEMP_DIR}/home"
export UPDATE_LOG="${TEMP_DIR}/update.log"
export ZENBOOK_DUO_GRUB_DROPIN="${TEMP_DIR}/grub.d/91-zenbook-duo-edp1-orientation.cfg"
export ZENBOOK_DUO_DMI_VENDOR_PATH="${TEMP_DIR}/vendor"
export ZENBOOK_DUO_DMI_PRODUCT_PATH="${TEMP_DIR}/product"
export ZENBOOK_DUO_CMDLINE_PATH="${TEMP_DIR}/cmdline"
export ZENBOOK_DUO_MONITORS_XML="${TEMP_DIR}/home/.config/monitors.xml"
export ZENBOOK_DUO_GDM_MONITORS_XML="${TEMP_DIR}/gdm/monitors.xml"
export ZENBOOK_DUO_UPDATE_GRUB="${TEMP_DIR}/bin/update-grub"
unset SUDO_USER

"${SCRIPT}" install >/dev/null
grep -q 'video=eDP-1:panel_orientation=upside_down' "${ZENBOOK_DUO_GRUB_DROPIN}"
grep -q '<rotation>normal</rotation>' "${ZENBOOK_DUO_MONITORS_XML}"
grep -q '<rotation>normal</rotation>' "${ZENBOOK_DUO_GDM_MONITORS_XML}"
! grep -q 'upside_down' "${ZENBOOK_DUO_MONITORS_XML}"

GRUB_CMDLINE_LINUX_DEFAULT="quiet splash video=eDP-1:panel_orientation=left_side_up"
# shellcheck disable=SC1090
. "${ZENBOOK_DUO_GRUB_DROPIN}"
test "${GRUB_CMDLINE_LINUX_DEFAULT}" = "quiet splash video=eDP-1:panel_orientation=upside_down"

status="$("${SCRIPT}" status)"
grep -q '^machine-supported=yes$' <<< "${status}"
grep -q '^grub-configured=yes$' <<< "${status}"
grep -q '^running-kernel-active=no$' <<< "${status}"

printf 'quiet splash video=eDP-1:panel_orientation=upside_down\n' > "${TEMP_DIR}/cmdline"
grep -q '^running-kernel-active=yes$' < <("${SCRIPT}" status)

echo "PASS"
