# vsftpd lab server: plain FTP + explicit FTPS (vsftpd.conf) or implicit FTPS (vsftpd-implicit.conf).
# The self-signed certificate is generated at build time and never leaves the image.
FROM ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends vsftpd openssl \
    && rm -rf /var/lib/apt/lists/* \
    && openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=rxt-lab-ftp \
        -keyout /etc/ssl/private/lab-ftp.pem -out /etc/ssl/private/lab-ftp.pem \
    && chmod 600 /etc/ssl/private/lab-ftp.pem \
    && useradd -m -s /bin/bash alice && echo 'alice:lab-alice-pass' | chpasswd \
    && mkdir -p /var/run/vsftpd/empty
COPY vsftpd.conf vsftpd-implicit.conf /etc/
ENTRYPOINT ["/usr/sbin/vsftpd"]
CMD ["/etc/vsftpd.conf"]
