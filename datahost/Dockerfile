# syntax=docker/dockerfile:1
#
# inilla-datahost: serves the client's Data folder to the iNilla app (see src/main.rs).
# Build context is this directory.

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/inilla-datahost /usr/local/bin/inilla-datahost

FROM alpine:3
RUN adduser -S -D -H -u 10001 datahost \
    && mkdir -p /state \
    && chown datahost /state
COPY --from=build /usr/local/bin/inilla-datahost /usr/local/bin/inilla-datahost
USER datahost
EXPOSE 8099
ENTRYPOINT ["inilla-datahost"]
CMD ["serve"]
