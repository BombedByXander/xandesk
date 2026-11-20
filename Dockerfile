FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p arcdesk-relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home xandesk
COPY --from=build /src/target/release/arcdesk-relay /usr/local/bin/arcdesk-relay
USER xandesk
ENV ARCDESK_BIND=0.0.0.0:10000
EXPOSE 10000
CMD ["/usr/local/bin/arcdesk-relay"]
