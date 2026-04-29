FROM docker.io/library/rust:1.94-bookworm

WORKDIR /workspace

ENV CARGO_TARGET_DIR=/workspace/target
ENV CARGO_INCREMENTAL=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*

CMD ["cargo", "run", "--release"]
