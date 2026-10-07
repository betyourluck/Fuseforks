# Fuseforks を GUI なしで回す像（Spec 65 D7）。
#
# ビルドの文脈はリポジトリの直下（compose.yaml の build.context: ..）。除外は隣の
# Dockerfile.dockerignore（BuildKit が Dockerfile と同じ名前の .dockerignore を読む）。
# 村は像に焼かない — /data はマウントする（README の「置き場」）。

ARG RUST_IMAGE=rust:1-bookworm

FROM ${RUST_IMAGE} AS builder
# aws-lc-sys（rustls）が cmake と clang、keyring が libdbus を要る（P0 で確かめた最小の集合）。
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake clang libdbus-1-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# .git を送らないので、版番号は外から渡す（apps/cli/build.rs が git より先に見る）。
# 渡さなければ 0.0.0 になる（打っていないリリースを名乗らない）。
ARG FUSEFORKS_CLI_VERSION=
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    FUSEFORKS_CLI_VERSION="${FUSEFORKS_CLI_VERSION}" \
    cargo build -p fuseforks-cli --release --locked \
 && cp target/release/fuseforks-cli /usr/local/bin/fuseforks-cli

FROM debian:bookworm-slim
# tini: run と MCP の stdio が起こす孫のゾンビを刈る PID 1。
# libdbus-1-3: keyring の feature を落とさないため（--secrets env だけを使う像でもリンクに要る）。
# git / curl / bash / python / ssh / rg: run の許可コマンドと前判定でよく使うもの。足りない道具は
# この像を FROM した派生の像で足す（基本の像に積むと、使わない人の像が大きくなる）。
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates tzdata tini libdbus-1-3 \
      git curl bash python3 python-is-python3 openssh-client ripgrep \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -u 10001 -m fuseforks \
 && mkdir -p /data /work \
 && chown 10001:10001 /data /work
COPY --from=builder /usr/local/bin/fuseforks-cli /usr/local/bin/fuseforks-cli
USER 10001
ENV HOME=/home/fuseforks
# TZ は既定で設定しない — 他の地域の運用者の予定が黙ってずれる（D6）。.env で bake.json の
# sourceTimeZone を書く。
ENTRYPOINT ["tini", "--", "fuseforks-cli"]
CMD ["serve", "--data-dir", "/data", "--secrets", "env", "--start", "batch"]
