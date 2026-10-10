# Static musl build, shipped in an empty image.
FROM rust:1.98-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked && strip target/release/databasus-exporter

FROM scratch
COPY --from=build /src/target/release/databasus-exporter /databasus-exporter
USER 65534:65534
EXPOSE 9810
ENTRYPOINT ["/databasus-exporter"]
