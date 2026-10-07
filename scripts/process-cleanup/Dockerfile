# syntax=docker/dockerfile:1
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN --mount=type=secret,id=proxy_ca,required=true \
    --mount=type=secret,id=system_ca,required=true \
    cat /run/secrets/system_ca /run/secrets/proxy_ca > /tmp/build-ca.pem && \
    sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources && \
    apt-get -o Acquire::https::CaInfo=/tmp/build-ca.pem update && \
    apt-get -o Acquire::https::CaInfo=/tmp/build-ca.pem install -y --no-install-recommends python3-minimal libgcc-s1 ca-certificates && \
    rm -rf /var/lib/apt/lists/* /tmp/build-ca.pem
ENTRYPOINT ["/usr/bin/python3", "/probe/container-init.py"]
