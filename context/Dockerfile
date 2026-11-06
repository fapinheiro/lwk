# fapinheiro/lwk/context/Dockerfile — imagem de CI/build
# Base	rust:1.85.0 (imagem oficial Rust, ~1.5GB)
# Propósito	Compilar o código-fonte e rodar testes
# Contém	Rust toolchain, elementsd, electrs, dotnet, Node, Go, uniffi bindings, cargo tools
# Resultado	Ambiente de desenvolvimento completo
# O que roda	cargo test, geração de fixtures
# Produção?	Não — nunca vai a produção
# 
# tappix-api/DockerfilePrd — imagem de produção
# Base	debian:13-slim (imagem mínima, ~80MB)
# Propósito	Rodar o lwk_cli como servidor em produção
# Contém	Apenas lwk_cli compilado + jq + curl
# Resultado	Binário único exposto na porta 32112
# O que roda	lwk_cli server start
# Produção?	Sim — é essa que vai no servidor

FROM --platform=linux/amd64 rust:1.85.0@sha256:0ff31c9ffa641a62e48d543fb00b4960955ea375f40776f40f585b89e654cc5e

RUN apt update -qq
RUN apt upgrade --no-install-recommends -yqq

# libxml2-utils for xmllint
# libudev-dev for jade serial
# clang for wasm
# libtinfo5 libncurses5 for swift
# chromium-driver for wasm tests
RUN apt install -y libudev-dev libxml2-utils clang libtinfo5 libncurses5 chromium-driver
RUN wget https://packages.microsoft.com/config/debian/12/packages-microsoft-prod.deb -O packages-microsoft-prod.deb \
 && echo "8434dcb8c346dc95fbd63dbece056c343704590b58b6a5c323d39acf52bf0b48  packages-microsoft-prod.deb" | sha256sum -c - \
 && dpkg -i packages-microsoft-prod.deb && rm packages-microsoft-prod.deb
RUN apt update -qq && apt install -y dotnet-sdk-10.0

RUN rustup component add rustfmt
RUN rustup component add clippy
RUN rustup component add llvm-tools-preview
RUN rustup target add wasm32-unknown-unknown
RUN rustup install nightly-2025-02-20 # for docs building

RUN cargo install cargo-audit@0.22.0 --locked
RUN cargo install cargo-nextest@0.9.100 --locked
RUN cargo install grcov@0.10.5 --locked
RUN cargo install just@1.53.0 --locked

# we don't need this, but it downloads and cache most of our dependency tree
RUN cargo install lwk_cli@0.18.1 --locked

WORKDIR /opt/lwk-bins
COPY download_bins.sh /opt/lwk-bins/download_bins.sh
RUN ./download_bins.sh

WORKDIR /workspace

# update the installation script with curl -fsSL https://get.docker.com -o get-docker.sh
COPY get-docker.sh /workspace/get-docker.sh
RUN sh ./get-docker.sh --version 29.9.3

# uniffi binding generators (C#, C++, Go) removed: incompatible with rustc 1.85.0
# (require rustc >= 1.88 via transitive icu/textwrap deps).
# Not needed for ./test.sh — add back when upgrading to rustc 1.88+.
