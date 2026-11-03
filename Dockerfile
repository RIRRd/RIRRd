FROM docker.io/library/rust:1.99.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
COPY LICENSE NOTICE.md THIRD_PARTY_NOTICES.md ./
COPY licenses ./licenses
RUN cargo build --locked --release

FROM docker.io/library/debian:12.15-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 rust-irrd \
    && useradd --uid 65532 --gid 65532 --home-dir /var/lib/rust-irrd \
        --no-create-home --shell /usr/sbin/nologin rust-irrd \
    && install -d -m 0755 /etc/rust-irrd \
    && install -d -m 0700 -o rust-irrd -g rust-irrd /var/lib/rust-irrd/state
COPY --from=build /build/target/release/rust-irrd /usr/local/bin/rust-irrd
COPY --from=build /build/LICENSE /build/NOTICE.md /build/THIRD_PARTY_NOTICES.md /usr/share/licenses/rust-irrd/
COPY --from=build /build/licenses/ /usr/share/licenses/rust-irrd/licenses/
WORKDIR /var/lib/rust-irrd/state
ENV TMPDIR=/var/lib/rust-irrd/state
USER 65532:65532
EXPOSE 4343/tcp
STOPSIGNAL SIGTERM
ENTRYPOINT ["/bin/sh", "-c", "umask 077; exec /usr/local/bin/rust-irrd --config /etc/rust-irrd/irrd.toml \"$@\"", "rust-irrd"]
CMD ["server"]
