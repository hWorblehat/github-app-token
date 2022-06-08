FROM rust:1.61 AS builder
ARG PLATFORM=x86_64-unknown-linux-gnu
RUN mkdir -p /usr/local/bin /work
WORKDIR /usr/src
# RUN apt-get update && \
#     apt-get dist-upgrade -y && \
#     apt-get install -y musl-tools && \
#     rustup target add x86_64-unknown-linux-musl
ENV RUSTFLAGS='-C target-feature=+crt-static'
ENV CARGO_HOME=/cargo

# Build and cache dependencies as layer
RUN --mount=type=cache,target=/cargo cargo init github-app-token
WORKDIR /usr/src/github-app-token
COPY Cargo.toml Cargo.lock .
RUN  --mount=type=cache,target=/cargo cargo build --release --target ${PLATFORM} \
    && rm -rf target/${PLATFORM}/release/.fingerprint/github-app-token-*

# Build actual program
COPY src src
RUN --mount=type=cache,target=/cargo cargo build --release --target ${PLATFORM}
RUN strip -o /usr/local/bin/github-app-token target/${PLATFORM}/release/github-app-token

FROM scratch
COPY --from=builder /work /work
ENV HOME=/work
WORKDIR /work
COPY --from=builder /usr/local/bin/github-app-token /usr/local/bin/github-app-token
ENTRYPOINT ["/usr/local/bin/github-app-token"]
