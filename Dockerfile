FROM rust as builder
ARG RUST_NIGHTLY_VERSION=nightly-2020-12-31

RUN rustup install ${RUST_NIGHTLY_VERSION} && rustup default ${RUST_NIGHTLY_VERSION}

WORKDIR /usr/local/src/fuzz-ci
COPY src src
COPY Cargo.* ./
RUN cargo install --path . --root /usr/local


FROM simplestakingcom/tezedge-ci-builder

USER root

ARG RUST_NIGHTLY_VERSION=nightly-2020-12-31
RUN rustup install ${RUST_NIGHTLY_VERSION} && rustup default ${RUST_NIGHTLY_VERSION}

RUN apt-get update && apt-get install -y \
    git libssl-dev curl build-essential binutils-dev libunwind-dev \
    libclang-dev \
    libblocksruntime-dev liblzma-dev \
    python3 libcurl4-openssl-dev libdw-dev libiberty-dev elfutils cmake \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install honggfuzz cargo-kcov

ARG KCOV_VERSION=38
RUN curl -L -o - https://github.com/SimonKagstrom/kcov/archive/v${KCOV_VERSION}.tar.gz | \
    zcat - | tar xf - -C /tmp && \
    mkdir /tmp/kcov-${KCOV_VERSION}/build && cd /tmp/kcov-${KCOV_VERSION}/build && \
    cmake .. && make && make install && rm -rf /tmp/kcov-${KCOV_VERSION}

#ENV PATH=$PATH:/root/.cargo/bin/

COPY --from=builder /usr/local/bin/fuzz-ci /usr/local/bin/fuzz-ci
CMD ["fuzz-ci", "-d", "server"]
