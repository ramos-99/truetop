# demo/demo.tape - reproducible truetop demo, rendered with charmbracelet/vhs.
# VHS renders its own clean terminal, so the GIF has no wallpaper/transparency.
#
# Prereqs (must run on a machine with a real disk - the recording is a real
# session, nothing is faked). Install as a plain `truetop` so the GIF shows the
# shipped command, not a dev path:
#   cargo build --release
#   sudo install -m 755 target/release/truetop /usr/local/bin/truetop
#   sudo setcap cap_bpf,cap_perfmon+ep /usr/local/bin/truetop   # no sudo prompt on camera
#   mkdir -p assets && vhs demo/demo.tape

Output assets/demo.gif

Set Shell bash
Set FontSize 15
Set Width 1280
Set Height 760
Set Padding 16
Set Theme "Catppuccin Mocha"
Set Framerate 24

# Stage the workload off-camera: O_DIRECT readers that genuinely block on the
# disk (D-state) plus one CPU hog. Let the I/O wait build before recording.
Hide
Type "./demo/demo-load.sh start" Enter
Sleep 3s
Ctrl+L
Show

# Launch (CAP_BPF-granted, so no sudo). Typed slowly so the command reads, then
# sorted by CPU first, like every other top.
Type@300ms "truetop" Enter
Sleep 4s

# Sort by I/O wait: the disk readers climb to the top - high IO Wait, low CPU.
# The processes stuck on the disk, the column no other top surfaces.
Type "i"
Sleep 5s

# Contrast: sort by CPU - the busy loop leads, the disk readers fall away.
# Same processes, a different truth.
Type "c"
Sleep 3s

# End on the differentiator.
Type "i"
Sleep 3s

Hide
Type "q"
Sleep 500ms
Type "./demo/demo-load.sh stop" Enter
Sleep 1s
Show
