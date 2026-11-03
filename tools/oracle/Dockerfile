FROM python:3.13.9-slim-bookworm@sha256:b685a4fa58bb19d1814d78a1ec0f0208f351452724f78b20212c984d6e124a34 AS build

RUN apt-get update && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*
RUN python -m venv /opt/irrd \
    && /opt/irrd/bin/pip install --no-cache-dir 'https://files.pythonhosted.org/packages/6b/6e/751a04df945c2169e68d8c98f2a9783ccab14903093b9eb348db601a4c0c/irrd-4.5.4-py3-none-any.whl#sha256=0ee7a0d64d8aef7c8b7ee0d2f7a24ff96c4ff1aa37b8d443cd2f3445ff939929'

FROM python:3.13.9-slim-bookworm@sha256:b685a4fa58bb19d1814d78a1ec0f0208f351452724f78b20212c984d6e124a34
LABEL org.opencontainers.image.source="https://github.com/irrdnet/irrd" \
      org.opencontainers.image.revision="8e8080dab744a348b7d0e6efcb53f403301dd075" \
      org.opencontainers.image.version="4.5.4"
RUN apt-get update && apt-get install -y --no-install-recommends gnupg \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system irrd \
    && useradd --system --gid irrd --home-dir /var/lib/irrd --create-home irrd \
    && install -d -m 0700 -o irrd -g irrd /var/lib/irrd/gnupg
COPY --from=build /opt/irrd /opt/irrd
COPY irrd.yaml /etc/irrd.yaml
ENV PATH="/opt/irrd/bin:${PATH}" PYTHONUNBUFFERED=1
USER irrd
WORKDIR /var/lib/irrd
