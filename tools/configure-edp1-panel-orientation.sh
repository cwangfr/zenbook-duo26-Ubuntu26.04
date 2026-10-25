#!/usr/bin/env bash
set -euo pipefail

PARAMETER="video=eDP-1:panel_orientation=upside_down"
EXPECTED_VENDOR="ASUS"
EXPECTED_PRODUCT="Zenbook Duo UX8407AA"

GRUB_DROPIN="${ZENBOOK_DUO_GRUB_DROPIN:-/etc/default/grub.d/91-zenbook-duo-edp1-orientation.cfg}"
DMI_VENDOR_PATH="${ZENBOOK_DUO_DMI_VENDOR_PATH:-/sys/class/dmi/id/sys_vendor}"
DMI_PRODUCT_PATH="${ZENBOOK_DUO_DMI_PRODUCT_PATH:-/sys/class/dmi/id/product_name}"
CMDLINE_PATH="${ZENBOOK_DUO_CMDLINE_PATH:-/proc/cmdline}"
USER_MONITORS="${ZENBOOK_DUO_MONITORS_XML:-${SUDO_USER:+/home/${SUDO_USER}/.config/monitors.xml}}"
USER_MONITORS="${USER_MONITORS:-${HOME}/.config/monitors.xml}"
GDM_MONITORS="${ZENBOOK_DUO_GDM_MONITORS_XML:-/var/lib/gdm3/seat0/config/monitors.xml}"

usage() {
    cat <<'EOF'
configure-edp1-panel-orientation.sh - advertise the upside-down eDP-1 mount

Usage:
  ./tools/configure-edp1-panel-orientation.sh install
  ./tools/configure-edp1-panel-orientation.sh status
  ./tools/configure-edp1-panel-orientation.sh remove

install writes a GRUB drop-in:
  video=eDP-1:panel_orientation=upside_down

Plymouth reads that property and rotates the splash. Mutter adds the same
180 degrees to the logical transform, so monitors.xml must say normal rather
than upside_down. install rewrites that rotation in the user and GDM files.

Reboot once, without logging out first. The installed runtime must already
be the build that sends logical transform "normal" for the upright image.
EOF
}

read_trimmed() {
    tr -d '\r\n' < "$1" 2>/dev/null || true
}

is_supported_machine() {
    [ "$(read_trimmed "${DMI_VENDOR_PATH}")" = "${EXPECTED_VENDOR}" ] &&
        [ "$(read_trimmed "${DMI_PRODUCT_PATH}")" = "${EXPECTED_PRODUCT}" ]
}

as_root() {
    if [ "${EUID}" -eq 0 ]; then
        "$@"
        return
    fi
    if ! command -v sudo >/dev/null 2>&1; then
        echo "ERROR: sudo is required to update GRUB." >&2
        exit 1
    fi
    sudo "$@"
}

resolve_update_grub() {
    if [ -n "${ZENBOOK_DUO_UPDATE_GRUB:-}" ]; then
        printf '%s\n' "${ZENBOOK_DUO_UPDATE_GRUB}"
        return
    fi
    command -v update-grub 2>/dev/null || true
}

render_dropin() {
    cat <<'EOF'
# Managed by zenbook-duo26-Ubuntu26.04.
# eDP-1 is mounted upside down. Plymouth rotates when the kernel says so.
_zenbook_duo_cmdline=""
for _zenbook_duo_arg in ${GRUB_CMDLINE_LINUX_DEFAULT:-}; do
    case "${_zenbook_duo_arg}" in
        video=eDP-1:panel_orientation=*)
            ;;
        *)
            _zenbook_duo_cmdline="${_zenbook_duo_cmdline}${_zenbook_duo_cmdline:+ }${_zenbook_duo_arg}"
            ;;
    esac
done
GRUB_CMDLINE_LINUX_DEFAULT="${_zenbook_duo_cmdline}${_zenbook_duo_cmdline:+ }video=eDP-1:panel_orientation=upside_down"
unset _zenbook_duo_arg _zenbook_duo_cmdline
EOF
}

normalize_monitors() {
    local file="$1"
    [ -f "${file}" ] || return 0
    as_root sed -i 's/<rotation>upside_down<\/rotation>/<rotation>normal<\/rotation>/' "${file}"
}

update_grub() {
    local update_command
    update_command="$(resolve_update_grub)"
    if [ -z "${update_command}" ]; then
        echo "ERROR: update-grub was not found; GRUB was not regenerated." >&2
        return 1
    fi
    as_root "${update_command}"
}

install_parameter() {
    if ! is_supported_machine; then
        echo "Skipping eDP-1 panel orientation: this machine is not ${EXPECTED_PRODUCT}."
        return
    fi

    local temp_file
    temp_file="$(mktemp)"
    render_dropin > "${temp_file}"
    as_root install -D -m 0644 "${temp_file}" "${GRUB_DROPIN}"
    rm -f "${temp_file}"
    normalize_monitors "${USER_MONITORS}"
    normalize_monitors "${GDM_MONITORS}"
    update_grub
    echo "Configured ${PARAMETER}."
    echo "Reboot once without logging out. The running runtime must already use logical transform normal."
}

remove_parameter() {
    if [ -e "${GRUB_DROPIN}" ]; then
        as_root rm -f "${GRUB_DROPIN}"
        update_grub
        echo "Removed the eDP-1 panel-orientation GRUB drop-in."
    else
        echo "No project-managed eDP-1 orientation drop-in is installed."
    fi
}

parameter_is_active() {
    [ -r "${CMDLINE_PATH}" ] && grep -Fqw -- "${PARAMETER}" "${CMDLINE_PATH}"
}

show_status() {
    local configured="no"
    local active="no"
    if [ -r "${GRUB_DROPIN}" ] && grep -Fq -- "${PARAMETER}" "${GRUB_DROPIN}"; then
        configured="yes"
    fi
    if parameter_is_active; then
        active="yes"
    fi
    printf 'machine-supported=%s\n' "$(is_supported_machine && echo yes || echo no)"
    printf 'grub-configured=%s\n' "${configured}"
    printf 'running-kernel-active=%s\n' "${active}"
    printf 'parameter=%s\n' "${PARAMETER}"
}

case "${1:-status}" in
    install) install_parameter ;;
    status) show_status ;;
    remove) remove_parameter ;;
    -h|--help|help) usage ;;
    *) usage >&2; exit 2 ;;
esac
