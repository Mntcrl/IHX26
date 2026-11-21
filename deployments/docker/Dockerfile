FROM --platform=$BUILDPLATFORM rust:1.91.1-bookworm@sha256:c1e5f19e773b7878c3f7a805dd00a495e747acbdc76fb2337a4ebf0418896b33 AS build
ARG TARGETARCH
ENV CARGO_BUILD_JOBS=2
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake ca-certificates \
    && if [ "$TARGETARCH" = arm64 ] && [ "$(dpkg --print-architecture)" != arm64 ]; then \
         apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu g++-aarch64-linux-gnu; \
       elif [ "$TARGETARCH" = amd64 ] && [ "$(dpkg --print-architecture)" != amd64 ]; then \
         apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu; \
       fi \
    && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu ;; \
      arm64) echo aarch64-unknown-linux-gnu ;; \
      *) echo "Unsupported target architecture" >&2; exit 1 ;; \
    esac > /tmp/galaxy-target && rustup target add "$(cat /tmp/galaxy-target)"
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=galaxy-node-cargo-${TARGETARCH},sharing=locked \
    set -eu; \
    target="$(cat /tmp/galaxy-target)"; \
    arch="${target%%-*}"; \
    if [ "$(rustc -vV | grep '^host: ' | cut -d' ' -f2)" != "$target" ]; then \
      export "CC_${arch}_unknown_linux_gnu=$arch-linux-gnu-gcc"; \
      export "CXX_${arch}_unknown_linux_gnu=$arch-linux-gnu-g++"; \
      export "CARGO_TARGET_$(echo "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER=$arch-linux-gnu-gcc"; \
    fi; \
    cargo build --locked --release --target "$target" -p galaxy-node --no-default-features; \
    install -Dm755 "target/$target/release/galaxy-node" /out/bin/galaxy-node; \
    install -d -m700 /out/state

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
COPY --from=build /out/bin/galaxy-node /usr/local/bin/galaxy-node
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build --chown=10001:10001 /out/state /var/lib/galaxy
USER 10001:10001
EXPOSE 8080
VOLUME ["/var/lib/galaxy"]
HEALTHCHECK --interval=15s --timeout=5s --start-period=15s CMD ["galaxy-node", "health", "--url", "http://127.0.0.1:8080"]
ENTRYPOINT ["galaxy-node"]
CMD ["run", "--config", "/etc/galaxy/node.toml"]
