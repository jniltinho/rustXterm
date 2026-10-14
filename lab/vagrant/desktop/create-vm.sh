#!/bin/sh
# Create a clean VirtualBox desktop VM for the desktop matrix (docs/gates/release.md, 10.4):
#   sh lab/vagrant/desktop/create-vm.sh <flavor> <ssh-port> <key-dir>
#   sh lab/vagrant/desktop/create-vm.sh destroy <flavor>
# Flavors (official cloud images + cloud-init, autologin of the fictional user "tester"):
#   ubuntu2604  Ubuntu 26.04, GNOME 50 (Wayland; X11 through Xwayland)
#   fedora      Fedora 44, GNOME (Wayland; X11 through Xwayland)
#   arch        Arch Linux, KDE Plasma 6 (Wayland session + plasma-x11-session)
# Same approach as rust-microsip's scripts/vm/create-desktop-vm.sh: cloud images instead of
# Vagrant boxes because Arch publishes no VirtualBox box any more. Images are checked against the
# distribution's checksums (Ubuntu and Fedora signature-verified; Arch over HTTPS only).
# ponytail: Arch image signature not verified; add its signing key if needed.
# "tester" has a locked password and an SSH key in <key-dir> (keep it outside the repository).
# SSH is forwarded on 127.0.0.1:<ssh-port> only; clipboard and drag-and-drop with the host are off.
# The VM reaches the host's Docker lab (bound to 127.0.0.1) at 10.0.2.2: VirtualBox 7 NAT maps it to
# the host loopback only with --nat-localhostreachable1 on (off by default: "Network is unreachable").
# Graphics: VMSVGA without 3D, so wgpu runs on Mesa software rendering (lavapipe or llvmpipe GL).
set -eu
if [ "${1:-}" = destroy ]; then
    name="rustxterm-desk-${2:?flavor}"
    VBoxManage controlvm "$name" poweroff 2>/dev/null || true
    sleep 2
    VBoxManage unregistervm "$name" --delete
    exit 0
fi
flavor=${1:?flavor}; port=${2:?ssh port}; keys=${3:?key directory}
fedora=https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images
case "$flavor" in
    ubuntu2604)
        url=https://cloud-images.ubuntu.com/releases/26.04/release/ubuntu-26.04-server-cloudimg-amd64.img
        sums=https://cloud-images.ubuntu.com/releases/26.04/release/SHA256SUMS; check=ubuntu ;;
    fedora)
        url=$fedora/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2
        sums=$fedora/Fedora-Cloud-44-1.7-x86_64-CHECKSUM; check=fedora ;;
    arch)
        url=https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2
        sums=$url.SHA256; check=plain ;;
    *) echo "usage: $0 ubuntu2604|fedora|arch <ssh-port> <key-dir> | destroy <flavor>" >&2; exit 2 ;;
esac
# Test tools in every flavor: ydotool (input on Wayland and X11), wl-clipboard, xclip, xwininfo/xprop,
# ImageMagick (window crops), zenity (rfd fallback). Arch has no Vulkan driver on purpose, so wgpu
# must fall back to GL (llvmpipe); Ubuntu and Fedora ship lavapipe.
# "|" in the autologin content is a newline.
case "$flavor" in
    ubuntu2604) admin=sudo dm=gdm3
        install="apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y -o Dpkg::Options::=--force-confold ubuntu-desktop-minimal gdm3 ydotool wl-clipboard xclip x11-utils imagemagick zenity && apt-get remove -y gnome-initial-setup || true"
        autologin_path=/etc/gdm3/custom.conf
        autologin="[daemon]|AutomaticLoginEnable=true|AutomaticLogin=tester|WaylandEnable=true" ;;
    fedora) admin=wheel dm=gdm
        install="dnf -y install @workstation-product-environment ydotool wl-clipboard xclip xwininfo xprop ImageMagick zenity && dnf -y remove gnome-initial-setup"
        autologin_path=/etc/gdm/custom.conf
        autologin="[daemon]|AutomaticLoginEnable=true|AutomaticLogin=tester|WaylandEnable=true" ;;
    arch) admin=wheel dm=sddm
        install="pacman -Syu --noconfirm --needed plasma-desktop plasma-x11-session xorg-server sddm konsole dolphin breeze-gtk kwallet kwallet-pam kwalletmanager xdg-desktop-portal-kde mesa ydotool wl-clipboard xclip xorg-xwininfo xorg-xprop imagemagick zenity noto-fonts"
        autologin_path=/etc/sddm.conf.d/autologin.conf
        autologin="[Autologin]|User=tester|Session=plasma" ;;
esac
name="rustxterm-desk-$flavor"
base=$(VBoxManage list systemproperties | sed -n 's/^Default machine folder: *//p')
dir="$base/$name"
command -v git >/dev/null || { echo "git is needed to check <key-dir>" >&2; exit 2; }
mkdir -p "$keys"
if git -C "$keys" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "keep <key-dir> outside any Git work tree: it holds a private key" >&2; exit 2
fi
mkdir -p "$dir"
[ -f "$keys/id_vm" ] || ssh-keygen -q -t ed25519 -N '' -C rustxterm-desktop-vm -f "$keys/id_vm"
file=$(basename "$url")
curl -fsSL -o "$dir/$file" "$url"
curl -fsSL -o "$dir/SUMS" "$sums"
case "$check" in
    ubuntu)
        curl -fsSL -o "$dir/SUMS.gpg" "$sums.gpg"
        gpgv --keyring /usr/share/keyrings/ubuntu-cloudimage-keyring.gpg "$dir/SUMS.gpg" "$dir/SUMS" ;;
    fedora)
        curl -fsSL -o "$dir/fedora.gpg" https://fedoraproject.org/fedora.gpg
        gpg --dearmor < "$dir/fedora.gpg" > "$dir/fedora.kbx"
        gpgv --keyring "$dir/fedora.kbx" --output "$dir/SUMS.signed" "$dir/SUMS"
        mv "$dir/SUMS.signed" "$dir/SUMS"
        rm "$dir/fedora.gpg" "$dir/fedora.kbx" ;;
esac
# Ubuntu "<sum> *file", Arch "<sum>  file", Fedora "SHA256 (file) = <sum>".
sum=$(sed -nE "s/^([0-9a-f]{64}) [ *]?$file\$/\\1/p; s/^SHA256 \\($file\\) = ([0-9a-f]{64})\$/\\1/p" "$dir/SUMS" | head -1)
[ -n "$sum" ] || { echo "no checksum for $file" >&2; exit 1; }
printf '%s  %s\n' "$sum" "$file" | (cd "$dir" && sha256sum --check --strict -)
qemu-img convert -O vdi "$dir/$file" "$dir/disk.vdi"
rm -f "$dir/$file" "$dir/SUMS" "$dir/SUMS.gpg"
VBoxManage modifymedium disk "$dir/disk.vdi" --resize 40960 >/dev/null
seed=$(mktemp -d)
trap 'rm -rf "$seed"' EXIT
{
    echo "#cloud-config"
    echo "hostname: rxt-$flavor"
    echo "users:"
    echo "  - name: tester"
    echo "    gecos: Fictional Tester"
    echo "    shell: /bin/bash"
    echo "    groups: [$admin, video, input]"
    echo "    sudo: ALL=(ALL) NOPASSWD:ALL"
    echo "    lock_passwd: true"
    echo "    ssh_authorized_keys: [\"$(cat "$keys/id_vm.pub")\"]"
    echo "ssh_pwauth: false"
    echo "write_files:"
    echo "  - path: $autologin_path"
    echo "    content: |"
    printf '%s\n' "$autologin" | tr '|' '\n' | sed 's/^/      /'
    # No first-run wizard for the autologin user.
    echo "  - path: /home/tester/.config/gnome-initial-setup-done"
    echo "    owner: tester:tester"
    echo "    content: \"yes\""
    echo "    defer: true"
    echo "runcmd:"
    # The marker exists only when every step succeeded.
    echo "  - [sh, -c, '$install && chown -R tester: /home/tester/.config && systemctl set-default graphical.target && systemctl enable $dm && touch /var/tmp/provisioned']"
    echo "power_state: {mode: reboot, condition: true}"
} > "$seed/user-data"
printf 'instance-id: %s-1\nlocal-hostname: rxt-%s\n' "$name" "$flavor" > "$seed/meta-data"
genisoimage -quiet -output "$dir/seed.iso" -volid cidata -joliet -rock "$seed/user-data" "$seed/meta-data"
VBoxManage createvm --name "$name" --ostype Linux_64 --register >/dev/null
VBoxManage modifyvm "$name" --memory 4096 --cpus 2 --graphicscontroller vmsvga --vram 128 \
    --audio-enabled off --nic1 nat --nat-localhostreachable1 on --natpf1 "ssh,tcp,127.0.0.1,$port,,22" --boot1 disk --rtc-use-utc on \
    --clipboard-mode disabled --drag-and-drop disabled --mouse usbtablet
VBoxManage storagectl "$name" --name SATA --add sata --portcount 2
VBoxManage storageattach "$name" --storagectl SATA --port 0 --type hdd --medium "$dir/disk.vdi"
VBoxManage storageattach "$name" --storagectl SATA --port 1 --type dvddrive --medium "$dir/seed.iso"
VBoxManage startvm "$name" --type headless
echo "provisioning; ready when: ssh -i $keys/id_vm -p $port tester@127.0.0.1 test -f /var/tmp/provisioned"
