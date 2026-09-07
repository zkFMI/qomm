# syntax=docker/dockerfile:1.7

# Docker demo network for QOMM on the post-quantum tree.
#
# Build context is the qomm repository root (compose.yaml uses `context: ..`).
# Nothing is copied from sibling checkouts: the DeFMI Avalanche VM and its
# network launcher are cloned at the revision rust/Cargo.toml pins, and the
# MP-SPDZ patches come from the zkpi revision the same file pins.  Keep the
# three *_COMMIT arguments in step with rust/Cargo.toml.

ARG RUST_VERSION=1.97.1
ARG MP_SPDZ_COMMIT=9d809599ea6ce627216a389ca7d984fbb75d0cb9
# zkpi carries the MP-SPDZ patches, including the mandatory hybrid TLS one.
# Must equal the zkpi `rev` in rust/Cargo.toml.
ARG ZKPI_COMMIT=cc9cfa9c65ff3e065b926abdb08d0b7bfebb666c
# defmi provides `qomm-avalanche-vm` and the `defmi_network` launcher.
# Must equal the defmi `rev` in rust/Cargo.toml.
ARG DEFMI_COMMIT=bc543696c7504b332735c14ed2e5c33267848e78
# Empty means `nproc` of the build host.
ARG BUILD_JOBS=
# OpenSSL 3.5 is the first release with X25519MLKEM768 and ML-DSA. MP-SPDZ and
# the Rust binaries are compiled against its headers and resolve libssl.so.3
# from /opt/pqc-openssl at run time; the distribution's libssl only serves curl.
ARG OPENSSL_VERSION=3.5.5
ARG OPENSSL_SHA256=b28c91532a8b65a1f983b4c28b7488174e4a01008e29ce8e69bd789f28bc2a89

# ---------------------------------------------------------------------------
# MP-SPDZ, hybrid-TLS build (OpenSSL 3.5.5 + zkpi patches)
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS mp-spdz-builder
ARG MP_SPDZ_COMMIT
ARG ZKPI_COMMIT
ARG BUILD_JOBS
ARG OPENSSL_VERSION
ARG OPENSSL_SHA256
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl git build-essential clang cmake yasm python3 perl \
       libssl-dev libgmp-dev libsodium-dev libboost-dev libboost-thread-dev \
       libboost-filesystem-dev libboost-iostreams-dev libntl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl --location --fail --silent --show-error \
      "https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_VERSION}/openssl-${OPENSSL_VERSION}.tar.gz" \
      -o /tmp/openssl.tar.gz \
    && echo "${OPENSSL_SHA256}  /tmp/openssl.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/pqc-build && tar -xzf /tmp/openssl.tar.gz -C /opt/pqc-build \
    && cd "/opt/pqc-build/openssl-${OPENSSL_VERSION}" \
    && ./Configure shared no-tests --prefix=/opt/pqc-openssl --openssldir=/etc/ssl \
    && make -j"${BUILD_JOBS:-$(nproc)}" build_sw \
    && make install_sw \
    && install -m 0644 LICENSE.txt /opt/pqc-openssl/LICENSE.txt \
    && rm -rf /opt/pqc-build /tmp/openssl.tar.gz
ENV PATH=/opt/pqc-openssl/bin:${PATH}
ENV LD_LIBRARY_PATH=/opt/pqc-openssl/lib64
# The party binary carries an rpath to /opt/pqc-openssl/lib64, so it still
# finds the 3.5 libssl when the MPC node launches it with LD_LIBRARY_PATH set
# to the checkout alone.  `.pqc-tls.sha256` is the receipt qomm-mpc's
# engine_policy binds the engine to; the mpc-runtime stage re-checks it.
RUN git clone https://github.com/zkFMI/zkpi.git /tmp/zkpi \
    && git -C /tmp/zkpi checkout --detach "${ZKPI_COMMIT}" \
    && test "$(git -C /tmp/zkpi rev-parse HEAD)" = "${ZKPI_COMMIT}" \
    && git clone https://github.com/data61/MP-SPDZ.git /opt/MP-SPDZ \
    && git -C /opt/MP-SPDZ checkout --detach "${MP_SPDZ_COMMIT}" \
    && test "$(git -C /opt/MP-SPDZ rev-parse HEAD)" = "${MP_SPDZ_COMMIT}" \
    && git -C /opt/MP-SPDZ apply --check /tmp/zkpi/rust/qomm-mpc/patches/expose-machine-to-embedder.patch \
    && git -C /opt/MP-SPDZ apply /tmp/zkpi/rust/qomm-mpc/patches/expose-machine-to-embedder.patch \
    && git -C /opt/MP-SPDZ apply --check /tmp/zkpi/rust/qomm-mpc/patches/require-hybrid-tls.patch \
    && git -C /opt/MP-SPDZ apply /tmp/zkpi/rust/qomm-mpc/patches/require-hybrid-tls.patch \
    && grep -q qomm_after_run_callback /opt/MP-SPDZ/Processor/OnlineMachine.hpp \
    && printf '\nCFLAGS += -I/opt/pqc-openssl/include\nLDLIBS += -L/opt/pqc-openssl/lib64 -Wl,-rpath,/opt/pqc-openssl/lib64\n' >> /opt/MP-SPDZ/CONFIG.mine \
    && make -C /opt/MP-SPDZ -j"${BUILD_JOBS:-$(nproc)}" libSPDZ.so malicious-shamir-party.x \
    && cd /opt/MP-SPDZ \
    && sha256sum Networking/ssl_sockets.h libSPDZ.so malicious-shamir-party.x > .pqc-tls.sha256 \
    && rm -rf /tmp/zkpi
# Development-only seven-party PKI for the public Docker demo, signed with the
# post-quantum algorithm the patched engine requires.  A real deployment
# provisions one private key per operator through its own HSM or secret
# manager; QOMM never falls back to MP-SPDZ's -u plaintext mode.
RUN umask 077; mkdir -p /opt/MP-SPDZ/Player-Data && cd /opt/MP-SPDZ/Player-Data \
    && for node in 0 1 2 3 4 5 6; do \
        openssl req -x509 -newkey ML-DSA-65 -noenc -days 3650 \
          -subj "/CN=P${node}" -addext "subjectAltName=DNS:P${node}" \
          -keyout "P${node}.key" -out "P${node}.pem" || exit 1; \
       done \
    && openssl rehash . \
    && chmod 0600 P[0-6].key

# ---------------------------------------------------------------------------
# QOMM gateway, frontend, participant and MPC node binaries (this repository)
# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS rust-builder
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       build-essential clang cmake pkg-config protobuf-compiler libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=mp-spdz-builder /opt/pqc-openssl /opt/pqc-openssl
# The workspace pins `openssl` with the vendored feature; OPENSSL_NO_VENDOR
# makes openssl-sys link the 3.5.5 build above instead, so every Rust binary
# and the MP-SPDZ engine share one TLS library.
ENV OPENSSL_DIR=/opt/pqc-openssl
ENV OPENSSL_NO_VENDOR=1
ENV PKG_CONFIG_PATH=/opt/pqc-openssl/lib64/pkgconfig
ENV LD_LIBRARY_PATH=/opt/pqc-openssl/lib64
WORKDIR /source
COPY rust ./rust
# web.rs embeds qomm_demo/static/* with include_bytes!; react-flow.{js,css}
# are the prebuilt bundle committed in this repository.
COPY qomm_demo/static ./qomm_demo/static
# MP_SPDZ_ROOT stays unset: qomm-mpc then builds without the in-process engine,
# which the distributed deployment does not use (each MPC node spawns the
# stock `malicious-shamir-party.x` from the mpc-runtime image).  The targets
# are named one by one because src/bin/qomm_live_acceptance_report.rs is a
# `#[path]` module of qomm_live_acceptance, not a standalone binary.
RUN --mount=type=cache,id=qomm-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=qomm-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=qomm-rust-target,target=/source/rust/target \
    env -u MP_SPDZ_ROOT cargo build \
      --manifest-path rust/Cargo.toml --locked --release \
      -p qomm-demo \
      --bin qomm-demo \
      --bin qomm_frontend \
      --bin qomm_participant_node \
      --bin qomm_mpc_node \
      --bin qomm_live_acceptance \
    && install -d /out \
    && install -m 0755 \
      rust/target/release/qomm-demo \
      rust/target/release/qomm_frontend \
      rust/target/release/qomm_participant_node \
      rust/target/release/qomm_mpc_node \
      rust/target/release/qomm_live_acceptance \
      /out/

# ---------------------------------------------------------------------------
# DeFMI Avalanche VM and network launcher (zkFMI/defmi at the pinned revision)
# ---------------------------------------------------------------------------
FROM rust:${RUST_VERSION}-bookworm AS defmi-builder
ARG DEFMI_COMMIT
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       ca-certificates git build-essential clang cmake pkg-config protobuf-compiler libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN git clone https://github.com/zkFMI/defmi.git /source/defmi \
    && git -C /source/defmi checkout --detach "${DEFMI_COMMIT}" \
    && test "$(git -C /source/defmi rev-parse HEAD)" = "${DEFMI_COMMIT}" \
    && test -f /source/defmi/avalanche/defmivm/config/test-genesis.json
WORKDIR /source/defmi/rust
RUN --mount=type=cache,id=defmi-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=defmi-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=defmi-vm-target,target=/source/defmi/rust/target \
    env -u MP_SPDZ_ROOT cargo build --locked --release \
      -p defmi-avalanche-vm --bin qomm-avalanche-vm --bin defmi_network \
    && install -d /out \
    && install -m 0755 target/release/qomm-avalanche-vm target/release/defmi_network /out/

# ---------------------------------------------------------------------------
# Unmodified AvalancheGo and the network runner (pinned releases)
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS avalanche-download
ARG TARGETARCH
ARG AVALANCHEGO_VERSION=1.14.2
ARG ANR_VERSION=1.8.3
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "${TARGETARCH}" in \
      amd64) \
        avalanche_sha=daf7f2739f0790c033ad728af6ee29bcd2660baf5265844079eb60f22d101e46; \
        anr_sha=0944cc0d8dd9b893d0f150cbca41a270563ba2ac137403d4cd1658c4e6a00368 ;; \
      arm64) \
        avalanche_sha=0c4563c55659fd9923bd1ea9d3dbcd0a85668325091ffc4a259d0a137033ad0b; \
        anr_sha=60a6306a246c7a89092788fd254c9b574287bb0a83de7b01e196ea5883a4dcfb ;; \
      *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 2 ;; \
    esac; \
    mkdir -p /download/avalanchego /download/anr; \
    curl --location --fail --silent --show-error \
      "https://github.com/ava-labs/avalanchego/releases/download/v${AVALANCHEGO_VERSION}/avalanchego-linux-${TARGETARCH}-v${AVALANCHEGO_VERSION}.tar.gz" \
      -o /tmp/avalanchego.tar.gz; \
    echo "${avalanche_sha}  /tmp/avalanchego.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/avalanchego.tar.gz -C /download/avalanchego --strip-components=1; \
    curl --location --fail --silent --show-error \
      "https://github.com/ava-labs/avalanche-network-runner/releases/download/v${ANR_VERSION}/avalanche-network-runner_${ANR_VERSION}_linux_${TARGETARCH}.tar.gz" \
      -o /tmp/anr.tar.gz; \
    echo "${anr_sha}  /tmp/anr.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/anr.tar.gz -C /download/anr; \
    test -x /download/avalanchego/avalanchego; \
    test -x /download/anr/avalanche-network-runner

# ---------------------------------------------------------------------------
# Runtime: gateway, frontend, participant modules, acceptance client
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS app-runtime
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl libssl3 libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 qomm \
    && install -d -o qomm -g qomm -m 0700 /var/lib/qomm-participant
COPY --from=mp-spdz-builder /opt/pqc-openssl /opt/pqc-openssl
COPY --from=rust-builder /out/qomm-demo /usr/local/bin/qomm-demo
COPY --from=rust-builder /out/qomm_frontend /usr/local/bin/qomm-frontend
COPY --from=rust-builder /out/qomm_participant_node /usr/local/bin/qomm-participant-node
COPY --from=rust-builder /out/qomm_live_acceptance /usr/local/bin/qomm-live-acceptance
ENV LD_LIBRARY_PATH=/opt/pqc-openssl/lib64
RUN for bin in qomm-demo qomm-frontend qomm-participant-node qomm-live-acceptance; do \
      ! ldd "/usr/local/bin/${bin}" | grep -F 'not found' || exit 1; \
    done
USER qomm
WORKDIR /home/qomm

FROM app-runtime AS mpc-runtime
USER root
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       python3 libssl3 libgmp10 libgmpxx4ldbl libsodium23 libboost-thread1.74.0 libntl44 \
       libboost-filesystem1.74.0 libboost-iostreams1.74.0 \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -o qomm -g qomm -m 0700 /var/lib/qomm-mpc
COPY --from=rust-builder /out/qomm_mpc_node /usr/local/bin/qomm-mpc-node
# Owned by the service user: the node compiles the demo circuit with the
# official compile.py into Programs/ at start-up.
COPY --chown=10001:10001 --from=mp-spdz-builder /opt/MP-SPDZ /opt/MP-SPDZ
ENV MP_SPDZ_ROOT=/opt/MP-SPDZ
ENV LD_LIBRARY_PATH=/opt/pqc-openssl/lib64:/opt/MP-SPDZ
ENV PYTHONDONTWRITEBYTECODE=1
# The MP-SPDZ Makefile writes its own RUNPATH (/opt/MP-SPDZ/local/lib:/opt/MP-SPDZ)
# and drops the -rpath from CONFIG.mine, and the MPC node launches the party
# with LD_LIBRARY_PATH set to the checkout only. Putting the 3.5 libraries on
# that RUNPATH makes the party fail closed onto the hybrid build regardless
# of the environment it is started with.
RUN ln -s /opt/pqc-openssl/lib64/libssl.so.3 /opt/MP-SPDZ/libssl.so.3 \
    && ln -s /opt/pqc-openssl/lib64/libcrypto.so.3 /opt/MP-SPDZ/libcrypto.so.3 \
    && cd /opt/MP-SPDZ && sha256sum -c .pqc-tls.sha256 \
    && ! ldd /opt/MP-SPDZ/malicious-shamir-party.x | grep -F 'not found' \
    && ! ldd /opt/MP-SPDZ/libSPDZ.so | grep -F 'not found' \
    && ! ldd /usr/local/bin/qomm-mpc-node | grep -F 'not found' \
    && env -u LD_LIBRARY_PATH ldd /opt/MP-SPDZ/malicious-shamir-party.x \
       | grep -F 'libssl.so.3 => /opt/MP-SPDZ/libssl.so.3' \
    && test "$(readlink -f /opt/MP-SPDZ/libssl.so.3)" = /opt/pqc-openssl/lib64/libssl.so.3 \
    && /opt/pqc-openssl/bin/openssl version | grep -F 'OpenSSL 3.5' \
    && test -f /opt/MP-SPDZ/Player-Data/P6.pem
USER qomm

# ---------------------------------------------------------------------------
# Runtime: DeFMI L1 (5 AvalancheGo validators + Rust VM) behind one /rpc
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS defmi-runtime
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=5 update \
    && apt-get install -y --no-install-recommends \
       ca-certificates curl libssl3 libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 qomm \
    && install -d -o qomm -g qomm -m 0700 /var/lib/qomm-defmi
COPY --from=defmi-builder /out/qomm-avalanche-vm /usr/local/bin/qomm-avalanche-vm
# The launcher was renamed defmi_network upstream; compose still invokes it
# under the name the previous deployment used.
COPY --from=defmi-builder /out/defmi_network /usr/local/bin/qomm-defmi-network
COPY --from=avalanche-download /download/avalanchego/avalanchego /usr/local/bin/avalanchego
COPY --from=avalanche-download /download/anr/avalanche-network-runner /usr/local/bin/avalanche-network-runner
COPY --from=defmi-builder /source/defmi/avalanche/defmivm/config/test-genesis.json /etc/qomm-defmi/genesis.json
RUN ! ldd /usr/local/bin/qomm-avalanche-vm | grep -F 'not found' \
    && ! ldd /usr/local/bin/qomm-defmi-network | grep -F 'not found'
USER qomm
WORKDIR /home/qomm
EXPOSE 9650
ENTRYPOINT ["qomm-defmi-network"]
