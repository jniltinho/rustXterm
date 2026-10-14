# TigerVNC lab server: VncAuth with a fixed fictional password, one xterm on display :1.
# BlacklistThreshold: parallel lab tests authenticate concurrently from one address.
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        tigervnc-standalone-server tigervnc-tools xterm \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash vnc \
    && echo 'lab-vnc1' | vncpasswd -f > /etc/vnc-lab-passwd && chown vnc:vnc /etc/vnc-lab-passwd && chmod 600 /etc/vnc-lab-passwd
USER vnc
EXPOSE 5900
ENTRYPOINT ["/bin/sh", "-c", "Xtigervnc :1 -rfbport 5900 -localhost=0 -SecurityTypes VncAuth -PasswordFile /etc/vnc-lab-passwd -BlacklistThreshold 1000 -geometry 1024x768 -depth 24 & sleep 1; DISPLAY=:1 xterm & wait"]
