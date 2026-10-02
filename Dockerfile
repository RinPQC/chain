FROM rust:1.93.0-slim-bookworm@sha256:776861219cd851131c1cec3bbd7cbeb16b99a794048097eb69ad9682a8ed0d57 AS build
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config ca-certificates git && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY vendor ./vendor
RUN cargo build --release --locked -p rinpqc-node

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates python3 iptables && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 rinpqc && useradd --uid 10001 --gid 10001 --no-create-home rinpqc \
    && mkdir -p /state /devnet/node0 /devnet/node1 /devnet/node2 /devnet/node3 \
    && chown -R 10001:10001 /state /devnet && chmod 700 /state /devnet/node*
COPY --from=build /build/target/release/rinpqc-node /usr/local/bin/rinpqc-node
COPY scripts/devnet/container.py /usr/local/lib/rinpqc-devnet.py
USER 10001:10001
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
ENTRYPOINT ["python3", "/usr/local/lib/rinpqc-devnet.py"]
CMD ["node"]
