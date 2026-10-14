# rustXterm icon set

Original flat icons drawn for rustXterm (MIT). SVG sources live in `svg/`, PNG renders in the size
folders (`16`, `32`, `48`, `64`, `96`), and `catalog.json` maps icon names to files. Distribution
logos used by the distro icons are in `brands/` (see `ATTRIBUTION.md`).

Rules for every SVG:
- File `svg/<name>.svg`, `viewBox="0 0 32 32"`, no `width`/`height`, no text, no fonts, no images,
  no scripts, no external references, no gradients, no filters, no shadows, no opacity tricks.
- Flat solid fills; at most 4 colors per icon, from a small, consistent palette.
- **No rounded corners**: sharp, square corners on every rectangle and polygon (`rx`/`ry` not used,
  `stroke-linejoin="miter"`, `stroke-linecap="butt"` or `square`). Circles and arcs are fine where the
  object itself is round (a disc, a gear, a globe).
- Must read at 16 px: few, bold shapes; lines at least 2 units thick; keep a 1-unit margin.
- Variants of one object (folder colors, monitor badges, `ft-*` file types) share the same base
  drawing and differ only in color or in a small badge in the lower-right corner.
- Transparent background.

Exceptions:
- Linux, Tux and distribution icons (`tux*`, `*-tux*`, `debian`, `redhat`, `raspberry*`, `*-suse`,
  `*-ubuntu`, `*-centos`, `*-fedora`, `*-freebsd`, `*-android`, `folder-linux` and the distro folders)
  must be faithful to the distribution logo with its real shape, proportions and
  colors (Tux with its black body, white belly, yellow beak and feet; Debian red swirl; Red Hat red
  fedora; Fedora blue "f" disc; SUSE green chameleon; Ubuntu orange circle of friends; Raspberry Pi
  raspberry; CentOS four-color pinwheel; FreeBSD red daemon ball; Android green robot). Keep the
  monitor/terminal/folder base of the flat set around them.
- People icons (`session`, `users`, `*-user`, `*-users`, `user-key`) use a simple
  figure with rounded shapes: round heads, rounded shoulders and bodies.
