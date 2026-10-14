# Telnet lab server (design T8): inetutils telnetd under inetutils-inetd with the fictional user
# alice / lab-alice-pass, plus socat for the raw TCP echo service (same image, other command).
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        inetutils-telnetd inetutils-inetd login socat \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash alice \
    && echo 'alice:lab-alice-pass' | chpasswd \
    && echo 'telnet stream tcp nowait root /usr/sbin/telnetd telnetd' > /etc/inetd.conf \
    && sed -i 's/^\(session\s\+\)required\(\s\+pam_loginuid.so\)/\1optional\2/' /etc/pam.d/login
EXPOSE 23 7
# -d: foreground with debug output.
CMD ["/usr/sbin/inetutils-inetd", "-d", "/etc/inetd.conf"]
