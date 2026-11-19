# Nessa's runtime on Agent37: the template thread VMs are made from.
# Built on Agent37's own base so the binaries, built on an instance of the
# same base by build.sh, run against the same system libraries. The base's
# ENTRYPOINT is kept: Agent37 injects its SSH server and boot hooks there.
FROM ghcr.io/agent37-platform/base:2026.10.24a
USER root
COPY opt/nessa /opt/nessa
RUN chmod -R a+rX /opt/nessa && chmod a+x /opt/nessa/bin/*
USER node
