# Tidy NAS agent (`tidy serve`, SPEC §14.13). Build from the repo root:
#   docker build -f docker/agent.Dockerfile -t tidy-agent .
# Volumes: /config (config.toml, rules.toml, tidy.db, agent-token) and
# /data (the NAS folders to organize). Port 7841.

ARG RUST_IMAGE=rust:1-bookworm
ARG BASE_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
RUN cargo build --release --locked -p tidy-cli \
 && cp target/release/tidy /usr/local/bin/tidy

FROM ${BASE_IMAGE}
# OCR (M15) on the NAS: tesseract + Korean, poppler for scanned PDFs.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates tini tesseract-ocr tesseract-ocr-kor poppler-utils \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/tidy /usr/local/bin/tidy
ENV TIDY_CONFIG=/config/config.toml \
    TIDY_DATA_ROOT=/data \
    TIDY_LISTEN=0.0.0.0:7841 \
    LANG=C.UTF-8
VOLUME ["/config", "/data"]
EXPOSE 7841
HEALTHCHECK --interval=60s --timeout=5s --start-period=20s \
  CMD ["/bin/sh", "-c", "exec 3<>/dev/tcp/127.0.0.1/7841 || exit 1"]
ENTRYPOINT ["/usr/bin/tini", "--", "tidy"]
CMD ["serve"]
LABEL org.opencontainers.image.source="https://github.com/sjh714252-prog/tidy" \
      org.opencontainers.image.description="Tidy NAS agent: organizes and indexes NAS folders, answers the Tidy PC app" \
      org.opencontainers.image.licenses="MIT"
