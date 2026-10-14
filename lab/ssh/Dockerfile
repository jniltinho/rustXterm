# OpenSSH lab server (used for sshd, bastion and internal). Fictional credentials only.
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        openssh-server xauth x11-apps x11-utils \
    && rm -rf /var/lib/apt/lists/* /etc/ssh/ssh_host_* \
    && useradd -m -s /bin/bash alice && echo 'alice:lab-alice-pass' | chpasswd \
    && useradd -m -s /bin/bash -p '*' bob \
    && useradd -m -s /bin/bash carol && echo 'carol:lab-carol-pass' | chpasswd \
    && useradd -m -s /bin/bash dave && echo 'dave:lab-dave-pass' | chpasswd \
    && useradd -M -d / -s /usr/sbin/nologin sftponly && echo 'sftponly:lab-sftp-pass' | chpasswd \
    && mkdir -p /srv/sftp/upload /run/sshd /etc/ssh/authorized_keys \
    && chown sftponly:sftponly /srv/sftp/upload
COPY sshd_config /etc/ssh/sshd_config
RUN printf 'RXT LAB BANNER: fictional test server, authorized lab users only.\n' > /etc/issue.net
COPY entrypoint.sh /entrypoint.sh
EXPOSE 22
ENTRYPOINT ["/bin/sh", "/entrypoint.sh"]
