# Secret Service test runtime (task 7.1): D-Bus session bus + gnome-keyring. Fictional password only.
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends dbus dbus-daemon gnome-keyring \
    && rm -rf /var/lib/apt/lists/*
COPY run-inside.sh /run-inside.sh
ENTRYPOINT ["/bin/sh", "/run-inside.sh"]
