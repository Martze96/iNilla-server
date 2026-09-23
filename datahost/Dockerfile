# syntax=docker/dockerfile:1
#
# inilla-datahost: serves the client's Data folder, and the addons that go with it, to the iNilla
# app (see src/main.rs). Build context is this directory.

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/inilla-datahost /usr/local/bin/inilla-datahost

# pfQuest (github.com/shagu/pfQuest, MIT), the quest helper the app installs with the game data:
# one release package per language, pinned and checked against its hash.
FROM alpine:3 AS addons
RUN apk add --no-cache curl unzip
ARG PFQUEST_VERSION=7.0.1
ARG PFQUEST_ENUS_SHA256=bc91f1cd73a608e9083eaa87db6bfc06e9cbc2c48c9d180476ed238b20e669a0
ARG PFQUEST_DEDE_SHA256=89346a185fa849311db998db5ee620eed4bd80deedfb822886f023da36c3b1e7
RUN for pair in "enUS ${PFQUEST_ENUS_SHA256}" "deDE ${PFQUEST_DEDE_SHA256}"; do \
      set -- $pair; \
      curl -fsSL -o /tmp/pfQuest.zip \
        "https://github.com/shagu/pfQuest/releases/download/${PFQUEST_VERSION}/pfQuest-$1.zip"; \
      echo "$2  /tmp/pfQuest.zip" | sha256sum -c -; \
      mkdir -p "/addons/$1" && unzip -q /tmp/pfQuest.zip -d "/addons/$1"; \
      rm /tmp/pfQuest.zip; \
    done

FROM alpine:3
RUN adduser -S -D -H -u 10001 datahost \
    && mkdir -p /state \
    && chown datahost /state
COPY --from=build /usr/local/bin/inilla-datahost /usr/local/bin/inilla-datahost
COPY --from=addons /addons /opt/inilla/addons
USER datahost
EXPOSE 8099
ENTRYPOINT ["inilla-datahost"]
CMD ["serve", "--data", "/client/Data"]
