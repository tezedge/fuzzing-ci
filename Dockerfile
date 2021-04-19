FROM rust as builder
ARG RUST_NIGHTLY_VERSION=nightly-2020-12-31

RUN rustup install ${RUST_NIGHTLY_VERSION} && rustup default ${RUST_NIGHTLY_VERSION}

WORKDIR /usr/local/src/fuzz-ci
COPY . .
RUN cargo version
RUN cargo install --path .

FROM debian:buster-slim
RUN apt-get update && apt-get install -y libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/fuzz-ci /usr/local/bin/fuzz-ci
CMD ["fuzz-ci", "server"]
