# Test VMs (create on demand, destroy afterwards)

VMs are disposable: create one for a gate or an investigation, record the result in the receipt
(`docs/gates/*.md`), then destroy it. Everything needed to rebuild them lives here. All accounts are
fictional, forwards bind to 127.0.0.1 only, and the VMs reach the host's Docker lab at `10.0.2.2`.

| VM | Purpose | Create | Destroy |
|---|---|---|---|
| `rustxterm-desk-<flavor>` | Desktop matrix (task 10.4): `ubuntu2604` GNOME, `fedora` GNOME, `arch` Plasma 6 | `sh lab/vagrant/desktop/create-vm.sh <flavor> <ssh-port> <key-dir>` | `sh lab/vagrant/desktop/create-vm.sh destroy <flavor>` |
| `rustxterm-windows` | Windows Server 2022: RDP against a real Windows host (NLA/CredSSP), Windows smoke/receipts (add-windows-port W1/W3) | `cd lab/vagrant/windows && vagrant up` | `cd lab/vagrant/windows && vagrant destroy -f` |
| `debian` (Vagrant default) | sshd on a full Debian host | `cd lab/vagrant/debian && vagrant up` | `vagrant destroy -f` |

## Windows VM (`windows`)

- Box `gusztavvargadr/windows-server-2022-standard` (evaluation; accept Microsoft's terms yourself),
  box account `vagrant`/`vagrant`, fictional RDP user `alice`. Forward: RDP 53389 on 127.0.0.1
  (used by `crates/rdp/tests/vm_rdp.rs`).
- Driver `rxt.sh` (rustXterm: `push` the Windows build to `C:\rxt`, `it` runs PowerShell in the
  desktop session, `click`, `keys`, `key`, `shot`). Screenshots go to the git-ignored `docs/prints/`.
  `RXT_WIN_VM` overrides the VirtualBox VM name (default `rustxterm-windows`).

## Desktop matrix VMs (`desktop`)

- Official cloud images plus cloud-init, checksum-verified; the fictional user `tester` logs in
  automatically and has a locked password and an SSH key from `<key-dir>` (keep it outside the repo).
- `vm-run.sh` runs a command in the graphical session, `ydo.sh` sends input through ydotool.
- The recipe and the last results are in `docs/gates/release.md` ("Desktop matrix").

## Disk

A Windows VM takes about 25 GB; a desktop VM 2–10 GB. Boxes are
cached in `~/.vagrant.d/boxes` (`vagrant box prune` removes old versions).
