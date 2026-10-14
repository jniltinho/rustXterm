# xrdp lab server: user alice gets a bare xterm session on xorgxrdp.
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        xrdp xorgxrdp xterm dbus-x11 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash alice && echo 'alice:lab-alice-pass' | chpasswd \
    && h=$(getent passwd alice | cut -d: -f6) && echo 'exec xterm' > "$h/.xsession" && chown alice:alice "$h/.xsession"
EXPOSE 3389
ENTRYPOINT ["/bin/sh", "-c", "rm -f /var/run/xrdp/*.pid; xrdp-sesman && exec xrdp --nodaemon"]
